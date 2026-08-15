package com.hdrconverter

import java.io.File

/**
 * CUDA 加速 JNI 桥
 *
 * 通过 System.load 加载 `backend/cuda/hdr_gpu_jni.dll`，
 * 用 GPU 加速像素级运算（Display-P3 转换 / 增益图 / HDR 变换）。
 *
 * 设计要点：
 *  - GPU 不可用时 init() 返回 false，所有 native 调用返回 false，调用方回退 CPU
 *  - 原生内核与 Kotlin CPU 实现逐位对齐，保证输出完全一致
 *  - /backend 接口据此上报 method=cuda（可用时）
 */
object HdrGpuJni {

    @Volatile private var initialized = false
    @Volatile private var available = false
    @Volatile private var gpuName = ""

    /** CUDA 是否可用（会触发一次懒加载初始化） */
    val isAvailable: Boolean
        get() {
            if (!initialized) init()
            return available
        }

    /** GPU 名称（不可用时为空串） */
    val name: String get() = gpuName

    /** 初始化 CUDA。幂等，可重复调用。 */
    fun init(): Boolean {
        if (initialized) return available
        synchronized(this) {
            if (initialized) return available
            initialized = true
            // 允许用户强制禁用 GPU（用于对比测试 / 排查问题）
            if (System.getenv("HDR_GPU_DISABLE") == "1" || System.getProperty("hdr.gpu.disable") == "true") {
                System.err.println("[HdrGpuJni] 已通过环境变量/系统属性禁用 GPU，使用 CPU")
                return false
            }
            try {
                val lib = locateDll()
                if (lib == null) {
                    System.err.println("[HdrGpuJni] 未找到 hdr_gpu_jni.dll，使用 CPU")
                    return false
                }
                System.load(lib)
                if (!nativeInit()) {
                    System.err.println("[HdrGpuJni] nativeInit 失败，使用 CPU")
                    return false
                }
                available = true
                gpuName = queryGpuName()
                return true
            } catch (e: Throwable) {
                System.err.println("[HdrGpuJni] CUDA 不可用，回退 CPU: ${e.message}")
                available = false
                return false
            }
        }
    }

    /** 定位 hdr_gpu_jni.dll（工作目录 / JAR 相对 / assets / 系统属性覆盖） */
    private fun locateDll(): String? {
        val candidates = LinkedHashSet<String>()
        val cwd = File(".").absoluteFile
        candidates += cwd.resolve("backend/cuda/hdr_gpu_jni.dll").path
        candidates += "backend/cuda/hdr_gpu_jni.dll"
        candidates += "assets/hdr_gpu_jni.dll"

        val jarDir = try {
            val url = HdrGpuJni::class.java.protectionDomain.codeSource.location
            val f = File(url.toURI())
            if (f.name.endsWith(".jar")) f.parentFile?.absoluteFile else null
        } catch (e: Exception) {
            null
        }
        if (jarDir != null) {
            // JAR 在 backend/kotlin/build/libs → ../../../../cuda = 项目根/backend/cuda（打包布局同理）
            candidates += jarDir.resolve("../../../../cuda/hdr_gpu_jni.dll").path
            candidates += jarDir.resolve("../../../cuda/hdr_gpu_jni.dll").path
            candidates += jarDir.resolve("../cuda/hdr_gpu_jni.dll").path
        }
        val prop = System.getProperty("hdr.gpu.jni")
        if (!prop.isNullOrBlank()) candidates += prop

        for (c in candidates) {
            if (File(c).exists()) return File(c).absolutePath
        }
        return null
    }

    private fun queryGpuName(): String {
        return try {
            // 通过 /backend 之外的方式无法直接取设备名，这里返回固定提示
            "NVIDIA CUDA"
        } catch (e: Throwable) {
            ""
        }
    }

    // ---------- 原生方法（对应 hdr_gpu_jni.cu） ----------

    private external fun nativeInit(): Boolean

    @Suppress("unused")
    private external fun nativeRelease()

    /**
     * sRGB RGBA → Display-P3（sRGB 传递）RGBA
     * @return true 表示成功写入 out
     */
    external fun nativeSrgbToDisplayP3(
        rgba: ByteArray, width: Int, height: Int, out: ByteArray
    ): Boolean

    /**
     * 计算高光扩展增益图
     * @param outGm8   输出 8-bit 增益图（n 字节）
     * @param outMinMax 输出 [minBoost, maxBoostActual]
     * @return true 表示成功
     */
    external fun nativeComputeGainMap(
        rgba: ByteArray, width: Int, height: Int,
        hdrIntensity: Double, gamma: Double,
        outGm8: ByteArray, outMinMax: DoubleArray
    ): Boolean

    /**
     * HDR 变换（PNG 路径）：sRGB → 线性 → 自动伽马 → 通道/曝光 → sRGB 编码
     * @return true 表示成功写入 out
     */
    external fun nativeApplyHdrTransform(
        rgba: ByteArray, width: Int, height: Int,
        totalExposure: Double, gamma: Double,
        rAdj: Double, gAdj: Double, bAdj: Double,
        out: ByteArray
    ): Boolean

    /**
     * HDR 变换 → Rec.2020/PQ 编码（png/jpg_icc 路径）：sRGB → 线性 → 自动伽马 → 通道/曝光
     * → 伽马 → Rec.709→Rec.2020 → PQ 编码（像素与 Rec.2020/PQ ICC 一致）
     * @return true 表示成功写入 out
     */
    external fun nativeApplyHdrTransformToRec2020Pq(
        rgba: ByteArray, width: Int, height: Int,
        totalExposure: Double, gamma: Double,
        rAdj: Double, gAdj: Double, bAdj: Double,
        whiteNits: Double, out: ByteArray
    ): Boolean
}
