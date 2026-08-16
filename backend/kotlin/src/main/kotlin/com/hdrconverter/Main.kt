package com.hdrconverter

import io.ktor.http.*
import io.ktor.serialization.kotlinx.json.*
import io.ktor.server.application.*
import io.ktor.server.engine.*
import io.ktor.server.netty.*
import io.ktor.server.plugins.contentnegotiation.*
import io.ktor.server.plugins.statuspages.*
import io.ktor.server.request.*
import io.ktor.server.response.*
import io.ktor.server.routing.*
import kotlinx.serialization.json.Json
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.awaitAll
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.withContext
import java.io.ByteArrayOutputStream
import java.io.File
import javax.imageio.ImageIO
import kotlin.concurrent.thread
import kotlin.system.exitProcess

/** 转换进度（供 /progress 轮询） */
object ConversionProgress {
    @Volatile
    var value: Double = 0.0

    @Volatile
    var active: Boolean = false

    @Volatile
    var message: String = "就绪"

    fun reset() {
        value = 0.0
        active = true
        message = "开始"
    }

    fun update(v: Double, msg: String) {
        value = v.coerceIn(0.0, 1.0)
        message = msg
    }

    fun finish() {
        value = 1.0
        active = false
        message = "完成"
    }
}

/**
 * 全局并发信号量：限制同时进行的转换任务数 = 核心数/2 + 1（至少 1）
 * 单张 /convert 与批量任务共用，保证总并发不超过容量。
 */
object ConversionSemaphore {
    val capacity: Int = maxOf(1, Runtime.getRuntime().availableProcessors() / 2 + 1)

    @PublishedApi
    internal val semaphore = java.util.concurrent.Semaphore(capacity, true)

    @PublishedApi
    internal val activeCount = java.util.concurrent.atomic.AtomicInteger(0)

    /** 当前正在执行的转换数 */
    val active: Int get() = activeCount.get()

    inline fun <T> withPermit(block: () -> T): T {
        semaphore.acquire()
        activeCount.incrementAndGet()
        try {
            return block()
        } finally {
            activeCount.decrementAndGet()
            semaphore.release()
        }
    }
}

/** 批量转换进度（供 /batch/progress 轮询） */
object BatchProgress {
    @Volatile
    var total: Int = 0
    val done = java.util.concurrent.atomic.AtomicInteger(0)
    val failed = java.util.concurrent.atomic.AtomicInteger(0)

    @Volatile
    var current: String = ""

    @Volatile
    var message: String = "就绪"

    @Volatile
    var running: Boolean = false

    // 逐项状态：inputPath -> queued|running|done|failed|cancelled
    val itemStatus = java.util.concurrent.ConcurrentHashMap<String, String>()

    fun reset(total: Int) {
        this.total = total
        this.done.set(0)
        this.failed.set(0)
        this.current = ""
        this.message = "准备批量转换"
        this.running = true
        itemStatus.clear()
    }

    fun jobStart(input: String) {
        current = input
        itemStatus[input] = "running"
        message = "转换中：${File(input).name}"
    }

    fun jobStage(input: String, stage: String) {
        message = "转换中：${File(input).name} · $stage"
    }

    fun jobDone(success: Boolean) {
        if (success) done.incrementAndGet() else failed.incrementAndGet()
        message = "已完成 ${done.get()}/${total}"
    }

    fun jobFinished(input: String, success: Boolean) {
        itemStatus[input] = if (success) "done" else "failed"
    }

    fun jobCancelled(input: String) {
        itemStatus[input] = "cancelled"
    }

    fun finish() {
        running = false
        message = "批量转换完成：成功 ${done.get()}，失败 ${failed.get()}"
    }
}

/** 批量取消：记录被取消的输入路径（待处理任务直接跳过，处理中任务在阶段间中止） */
object BatchCancel {
    private val cancelledInputs = java.util.concurrent.ConcurrentHashMap.newKeySet<String>()
    fun reset() {
        cancelledInputs.clear()
    }

    fun cancel(inputs: List<String>) {
        cancelledInputs.addAll(inputs)
    }

    fun isCancelled(input: String): Boolean = cancelledInputs.contains(input)
}

/** 单张转换取消：POST /cancel 置位，/convert 在阶段间检查（与批量取消互不影响） */
object SingleCancel {
    @Volatile
    var cancelled: Boolean = false
    fun reset() { cancelled = false }
    fun cancel() { cancelled = true }
    fun isCancelled(): Boolean = cancelled
}

/** 抛出后由 convertOne 捕获并返回「已取消」结果（不中断整个协程） */
class ConversionCancelled : RuntimeException("已取消")

/**
 * HDR Converter Backend - Kotlin 版
 *
 * 作为 HTTP 服务运行，供 Electron 主进程调用。
 * 启动时自动选择可用端口并打印到 stdout，供 Electron 读取。
 *
 * 接口:
 *   POST /convert   - 完整转换一张图片
 *   POST /preview   - 快速预览转换（缩放后处理，返回 base64 data URL）
 *   GET  /health    - 健康检查
 */
fun main() {
    val port = findAvailablePort(18765)
    val iccProfilePath = resolveIccProfilePath()

    // 验证 ICC 配置文件存在
    val iccFile = File(iccProfilePath)
    if (!iccFile.exists()) {
        System.err.println("错误: ICC 配置文件不存在: $iccProfilePath")
        System.exit(1)
    }
    val iccProfileBuffer = iccFile.readBytes()
    System.err.println("ICC 配置文件已加载: ${iccProfilePath} (${iccProfileBuffer.size} bytes)")

    // 初始化 CUDA 加速（GPU 不可用时自动回退 CPU）
    if (HdrGpuJni.init()) {
        System.err.println("[HdrGpuJni] CUDA 加速可用: ${HdrGpuJni.name}")
    } else {
        System.err.println("[HdrGpuJni] CUDA 不可用，使用 CPU 多线程")
    }

    val json = Json {
        ignoreUnknownKeys = true
        isLenient = true
        encodeDefaults = true
    }

    // 打印端口号到 stdout（Electron 主进程读取）
    println("HDR_BACKEND_PORT:$port")
    System.out.flush()

    val server = embeddedServer(Netty, port = port) {
        install(ContentNegotiation) {
            json(json)
        }

        install(StatusPages) {
            exception<Throwable> { call, cause ->
                val message = cause.message?.replace("\"", "\\\"") ?: "未知错误"
                call.respondText(
                    contentType = ContentType.Application.Json,
                    status = HttpStatusCode.InternalServerError
                ) {
                    """{"success":false,"message":"$message"}"""
                }
            }
        }

        /**
         * 单张转换（suspend）。内部通过全局信号量限制并发（核心数/2+1）。
         * 不抛异常：失败时返回 success=false 的 ConvertResponse。
         */
        suspend fun convertOne(
            inputPath: String,
            outputPath: String?,
            settings: ConversionSettings,
            onProgress: (Double, String) -> Unit,
            cancelCheck: (() -> Boolean)? = null
        ): ConvertResponse {
            val outputFormat = settings.outputFormat
            val ext = if (outputFormat == "png") ".png" else ".jpg"
            var out = outputPath ?: "output$ext"
            if (!out.lowercase().endsWith(ext)) out += ext
            val checkCancel: () -> Unit = {
                if (cancelCheck?.invoke() == true) throw ConversionCancelled()
            }
            return ConversionSemaphore.withPermit {
                try {
                    // 先检测原图色彩空间（依据 Ultra HDR 规范：SDR 色彩配置定义 HDR 色彩空间）
                    val detectedCs = ColorSpaceDetector.detect(inputPath)
                    System.err.println("[ColorSpaceDetector] ${inputPath} -> ${detectedCs.displayName}")
                    val file = withContext(Dispatchers.IO) {
                        checkCancel()
                        onProgress(0.05, "读取图片")
                        val imageData = HdrConverter.readImageAsRgba(inputPath)
                        checkCancel()
                        onProgress(0.10, "开始编码")
                        val resultBuffer = encodeAndInjectIcc(
                            imageData.pixels,
                            imageData.width,
                            imageData.height,
                            outputFormat,
                            iccProfileBuffer,
                            settings,
                            detectedCs
                        ) { v, msg ->
                            checkCancel()
                            onProgress(0.10 + 0.80 * v, msg)
                        }
                        checkCancel()
                        onProgress(0.95, "写入文件")
                        val f = File(out)
                        f.parentFile?.mkdirs()
                        f.writeBytes(resultBuffer)
                        f
                    }
                    ConvertResponse(
                        success = true,
                        outputPath = file.absolutePath,
                        outputFormat = outputFormat,
                        message = "转换完成，输出已保存",
                        detectedColorSpace = detectedCs.displayName
                    )
                } catch (e: ConversionCancelled) {
                    ConvertResponse(
                        success = false,
                        outputPath = out,
                        outputFormat = outputFormat,
                        message = "已取消"
                    )
                } catch (e: Throwable) {
                    ConvertResponse(
                        success = false,
                        outputPath = out,
                        outputFormat = outputFormat,
                        message = e.message ?: "转换失败"
                    )
                }
            }
        }

        routing {
            // 健康检查
            get("/health") {
                call.respond(mapOf("status" to "ok", "message" to "HDR Converter Backend is running"))
            }

            // 进度查询（供前端轮询）
            get("/progress") {
                call.respond(
                    mapOf(
                        "value" to ConversionProgress.value.toString(),
                        "active" to ConversionProgress.active.toString(),
                        "message" to ConversionProgress.message
                    )
                )
            }

            // 后端方式信息
            get("/backend") {
                val threads = Runtime.getRuntime().availableProcessors()
                val gpu = HdrGpuJni.isAvailable
                val method = if (gpu) "cuda" else "cpu"
                call.respond(
                    mapOf(
                        "method" to method,
                        "threads" to threads.toString(),
                        "message" to (if (gpu) "CUDA 加速（${HdrGpuJni.name}）" else "CPU 多线程（${threads} 核）")
                    )
                )
            }

            // 完整转换
            post("/convert") {
                val request = call.receive<ConvertRequest>()
                val settings = request.settings ?: ConversionSettings()
                SingleCancel.reset()
                ConversionProgress.reset()
                val resp = convertOne(
                    request.inputPath, request.outputPath, settings,
                    onProgress = { v, msg -> ConversionProgress.update(v, msg) },
                    cancelCheck = { SingleCancel.isCancelled() }
                )
                ConversionProgress.finish()
                if (!resp.success) throw RuntimeException(resp.message ?: "转换失败")
                call.respond(resp)
            }

            // 取消当前单张转换（尽力而为：处理中任务在阶段间中止，返回 success=false message=已取消）
            post("/cancel") {
                SingleCancel.cancel()
                call.respond(mapOf("ok" to "true"))
            }

            // 批量转换（并发受全局信号量限制 = 核心数/2+1）
            post("/batch/convert") {
                val request = call.receive<BatchConvertRequest>()
                val jobs = request.jobs
                BatchProgress.reset(jobs.size)
                BatchCancel.reset()
                if (jobs.isEmpty()) {
                    BatchProgress.finish()
                    call.respond(BatchConvertResponse(emptyList(), 0, 0))
                    return@post
                }
                val res = coroutineScope {
                    jobs.map { job ->
                        async(Dispatchers.IO) {
                            BatchProgress.jobStart(job.inputPath)
                            val r = if (BatchCancel.isCancelled(job.inputPath)) {
                                ConvertResponse(
                                    success = false,
                                    outputPath = job.outputPath,
                                    outputFormat = job.settings?.outputFormat ?: "jpg",
                                    message = "已取消"
                                )
                            } else {
                                convertOne(
                                    job.inputPath, job.outputPath, job.settings ?: ConversionSettings(),
                                    onProgress = { _, msg -> BatchProgress.jobStage(job.inputPath, msg) },
                                    cancelCheck = { BatchCancel.isCancelled(job.inputPath) }
                                )
                            }
                            if (r.message == "已取消") {
                                BatchProgress.jobCancelled(job.inputPath)
                            } else {
                                BatchProgress.jobFinished(job.inputPath, r.success)
                            }
                            BatchProgress.jobDone(r.success)
                            BatchJobResult(job.inputPath, r.outputPath, r.success, r.message)
                        }
                    }.awaitAll()
                }
                BatchProgress.finish()
                call.respond(
                    BatchConvertResponse(
                        results = res,
                        successCount = res.count { it.success },
                        failCount = res.count { !it.success }
                    )
                )
            }

            // 批量取消：标记要取消的输入路径（尽力而为）
            post("/batch/cancel") {
                val req = call.receive<BatchCancelRequest>()
                BatchCancel.cancel(req.inputPaths)
                call.respond(mapOf("ok" to "true"))
            }

            // 批量转换进度（供前端轮询，含逐项状态）
            get("/batch/progress") {
                call.respond(
                    BatchProgressResponse(
                        total = BatchProgress.total,
                        done = BatchProgress.done.get(),
                        failed = BatchProgress.failed.get(),
                        current = BatchProgress.current,
                        message = BatchProgress.message,
                        running = BatchProgress.running,
                        statuses = BatchProgress.itemStatus.entries.associate { it.key to it.value }
                    )
                )
            }

            // 后端状态（含并发容量）
            get("/status") {
                val threads = Runtime.getRuntime().availableProcessors()
                val gpu = HdrGpuJni.isAvailable
                call.respond(
                    mapOf(
                        "method" to (if (gpu) "cuda" else "cpu"),
                        "threads" to threads.toString(),
                        "capacity" to ConversionSemaphore.capacity.toString(),
                        "active" to ConversionSemaphore.active.toString(),
                        "gpuName" to HdrGpuJni.name,
                        "message" to (if (gpu) "CUDA 加速（${HdrGpuJni.name}）" else "CPU 多线程（${threads} 核）")
                    )
                )
            }

            // 预览转换
            post("/preview") {
                val request = call.receive<PreviewRequest>()
                val settings = request.settings ?: ConversionSettings()
                val outputFormat = settings.outputFormat

                ConversionProgress.reset()
                val result = ConversionSemaphore.withPermit {
                    withContext(Dispatchers.IO) {
                        try {
                            ConversionProgress.update(0.05, "读取图片")
                            val imageData = HdrConverter.readImageForPreview(request.inputPath, 0.5)
                            ConversionProgress.update(0.10, "开始编码")
                            val resultBuffer = if (request.mode == "videoDirect") {
                                // 视频直接转预览：与视频输出完全一致的 Rec.2020/PQ 色彩（曝光=峰值），修复预览色彩与视频不一致
                                val whiteNits = settings.whiteNits ?: 203.0
                                val peakNits = settings.peakNits ?: 1000.0
                                val peak = (peakNits / whiteNits).coerceAtLeast(1.0)
                                val rec2020Pq = UltraHdrEncoder.videoDirectPreviewRgba(
                                    imageData.pixels, imageData.width, imageData.height, settings, peak, whiteNits
                                )
                                val jpeg = HdrConverter.encodeJpeg(
                                    rec2020Pq, imageData.width, imageData.height,
                                    settings.quality.toFloat().coerceIn(0.1f, 1.0f)
                                )
                                IccInjector.injectIccIntoJpeg(jpeg, iccProfileBuffer)
                            } else {
                                encodeAndInjectIcc(
                                    imageData.pixels,
                                    imageData.width,
                                    imageData.height,
                                    outputFormat,
                                    iccProfileBuffer,
                                    settings
                                ) { v, msg -> ConversionProgress.update(0.10 + 0.85 * v, msg) }
                            }
                            val base64 = java.util.Base64.getEncoder().encodeToString(resultBuffer)
                            val mime = if (outputFormat == "png") "image/png" else "image/jpeg"
                            Triple("data:$mime;base64,$base64", imageData.width, imageData.height)
                        } finally {
                            ConversionProgress.finish()
                        }
                    }
                }

                call.respond(
                    PreviewResponse(
                        dataUrl = result.first,
                        width = result.second,
                        height = result.third,
                        aspectRatio = result.second.toDouble() / result.third
                    )
                )
            }

            // 自动估算 HDR 强度（基于亮度直方图，缩放到 25% 统计更快）
            post("/estimate") {
                val request = call.receive<EstimateRequest>()
                val result = withContext(Dispatchers.IO) {
                    val imageData = HdrConverter.readImageForPreview(request.inputPath, 0.25)
                    UltraHdrEncoder.estimateHdrIntensity(imageData.pixels, imageData.width, imageData.height)
                }
                call.respond(
                    EstimateResponse(
                        hdrIntensity = result.hdrIntensity,
                        maxBoost = result.maxBoost,
                        yP995 = result.yP995,
                        hlRatio = result.hlRatio,
                        message = "已自动估算 HDR 强度 " +
                                "%.2f".format(result.hdrIntensity) + " EV（maxBoost ×" +
                                "%.1f".format(result.maxBoost) + "）"
                    )
                )
            }

            // 视频逐帧重建（链路 2）：SDR 帧 → 线性 HDR 16-bit PAM
            // mode=gainmap（默认，增益图）| transform（图片 ICC 增益式单层）
            post("/video-frame") {
                val request = call.receive<VideoFrameRequest>()
                val settings = request.settings ?: ConversionSettings()
                val peak = request.peak ?: 8.0
                val mode = request.mode ?: "gainmap"
                ConversionProgress.reset()
                val result = withContext(Dispatchers.IO) {
                    ConversionProgress.update(0.05, "读取视频帧")
                    val imageData = HdrConverter.readImageAsRgba(request.inputPath)
                    ConversionProgress.update(
                        0.15,
                        if (mode == "transform") "单层 HDR 变换（ICC 增益式）" else "重建线性 HDR（增益图）"
                    )
                    val pam = if (mode == "transform") {
                        UltraHdrEncoder.reconstructLinearHdrTransform(
                            imageData.pixels, imageData.width, imageData.height, settings, peak
                        )
                    } else {
                        UltraHdrEncoder.reconstructLinearHdrFrame(
                            imageData.pixels, imageData.width, imageData.height, settings, peak
                        )
                    }
                    ConversionProgress.finish()
                    // outputPath 指定 → PAM 直接写文件，避免逐帧 base64 往返的大块传输开销
                    val outPath = request.outputPath
                    if (!outPath.isNullOrBlank()) {
                        File(outPath).let { f ->
                            f.parentFile?.mkdirs()
                            f.writeBytes(pam)
                        }
                        VideoFrameResponse(ok = true, width = imageData.width, height = imageData.height)
                    } else {
                        val base64 = java.util.Base64.getEncoder().encodeToString(pam)
                        VideoFrameResponse(pamBase64 = base64, width = imageData.width, height = imageData.height)
                    }
                }
                call.respond(result)
            }
        }
    }
    server.start(wait = false)

    // 自终止监视：Electron 主进程退出/崩溃时，stdin 管道写端关闭 → 读到 EOF → 自动关闭服务并退出，
    // 避免 portable 版遗留孤儿 JVM（占用端口/内存/GPU）。
    // 若 stdin 无效（如某些环境的 <nul 重定向 / 启动器不转发 stdin），则不启用监视，直接跳过。
    thread(name = "backend-stdin-watchdog") {
        val stdinUsable = try {
            System.`in`.available() >= 0
        } catch (_: Exception) {
            false
        }
        if (!stdinUsable) return@thread
        try {
            // Electron 从不写 stdin，这里只等待 EOF
            while (System.`in`.read() != -1) { /* 丢弃输入 */ }
        } catch (_: Exception) {
            /* 读取失败视为管道已关闭 */
        }
        System.err.println("[Backend] 父进程 stdin 管道已关闭（EOF），自动退出")
        runCatching { server.stop(gracePeriodMillis = 500, timeoutMillis = 5000) }
        exitProcess(0)
    }
}

/**
 * 编码原始输入为图片并注入 ICC / 生成 Ultra HDR JPEG
 *
 * @param originalRgba 原始输入 RGBA
 * @param onProgress   进度回调 (0..1, 消息)
 */
private fun encodeAndInjectIcc(
    originalRgba: ByteArray,
    width: Int,
    height: Int,
    format: String,
    iccProfile: ByteArray,
    settings: ConversionSettings,
    detectedCs: InputColorSpace? = null,
    onProgress: ((Double, String) -> Unit)? = null
): ByteArray {
    if (format == "png") {
        // PNG: 变换 + Rec.2020/PQ 编码（像素与注入的 Rec.2020/PQ ICC 一致，修复 sRGB 数值被当 Rec.2020/PQ 解释的色偏）
        onProgress?.invoke(0.8, "编码 HDR PNG")
        val whiteNits = settings.whiteNits ?: 203.0
        val transformed = HdrConverter.applyHdrTransformToRec2020Pq(originalRgba, width, height, settings, whiteNits)
        val img = HdrConverter.pixelsToBufferedImage(transformed, width, height)
        val baos = ByteArrayOutputStream()
        ImageIO.write(img, "png", baos)
        return IccInjector.injectIccIntoPng(baos.toByteArray(), iccProfile)
    }

    if (format == "jpg_icc") {
        // JPG（ICC 增益，BT.2020）：与 PNG 方案相同 —— 变换 + Rec.2020/PQ 编码 + 注入 BT.2020 ICC
        // ICC 以 APP2 段插到所有前置 APP 段之后（标准位置，勿插错）
        onProgress?.invoke(0.8, "编码 HDR JPEG")
        val whiteNits = settings.whiteNits ?: 203.0
        val transformed = HdrConverter.applyHdrTransformToRec2020Pq(originalRgba, width, height, settings, whiteNits)
        val quality = settings.quality.toFloat().coerceIn(0.1f, 1.0f)
        val jpeg = HdrConverter.encodeJpeg(transformed, width, height, quality)
        return IccInjector.injectIccIntoJpeg(jpeg, iccProfile)
    }

    // jpg: 生成符合 Ultra HDR 格式的 JPEG（SDR 底图 = 原始输入，增益图做高光扩展）
    // 传入检测到的输入色彩空间，由编码器决定主图/增益色彩空间（避免硬编码假设 sRGB）
    return UltraHdrEncoder.encode(originalRgba, width, height, settings, detectedCs, onProgress)
}

/**
 * 寻找可用端口
 */
private fun findAvailablePort(preferred: Int): Int {
    return try {
        val socket = java.net.ServerSocket(preferred)
        val port = socket.localPort
        socket.close()
        port
    } catch (e: Exception) {
        val socket = java.net.ServerSocket(0)
        val port = socket.localPort
        socket.close()
        port
    }
}

/**
 * 解析 ICC 配置文件路径
 * 优先查找与 JAR 同目录下的 2020_profile.icc
 * 其次查找相对路径
 */
private fun resolveIccProfilePath(): String {
    // 尝试 JAR 所在目录
    val jarDir = try {
        val url = IccInjector::class.java.protectionDomain.codeSource.location
        val jarFile = java.io.File(url.toURI())
        if (jarFile.name.endsWith(".jar")) jarFile.parentFile?.absolutePath
        else null
    } catch (e: Exception) {
        null
    }

    val candidates = listOfNotNull(
        jarDir?.let { "$it/../../../../assets/2020_profile.icc" },
        jarDir?.let { "$it/../../assets/2020_profile.icc" },
        jarDir?.let { "$it/assets/2020_profile.icc" },
        "assets/2020_profile.icc",
        "../assets/2020_profile.icc",
        "../../assets/2020_profile.icc",
        "2020_profile.icc"
    )

    for (path in candidates) {
        val file = File(path)
        if (file.exists()) return file.absolutePath
    }

    return "assets/2020_profile.icc"
}
