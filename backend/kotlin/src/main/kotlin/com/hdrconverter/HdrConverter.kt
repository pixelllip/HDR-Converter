package com.hdrconverter

import java.awt.RenderingHints
import java.awt.geom.AffineTransform
import java.awt.image.BufferedImage
import java.io.ByteArrayOutputStream
import java.io.File
import javax.imageio.IIOImage
import javax.imageio.ImageIO
import javax.imageio.ImageWriteParam
import javax.imageio.stream.MemoryCacheImageOutputStream

/**
 * HDR 转换核心逻辑
 *
 * 与原始 JS 版本保持算法一致：
 * sRGB → 线性 → 自动伽马 → RGB通道调整 → 曝光 → 伽马 → sRGB编码
 */
object HdrConverter {

    // ============================================================
    //  sRGB ↔ Linear 转换
    // ============================================================

    private fun srgbToLinear(value: Double): Double {
        return if (value <= 0.04045) value / 12.92
        else Math.pow((value + 0.055) / 1.055, 2.4)
    }

    private fun linearToSrgb(value: Double): Double {
        return if (value <= 0.0031308) value * 12.92
        else 1.055 * Math.pow(value, 1.0 / 2.4) - 0.055
    }

    private fun clamp(value: Double, min: Double, max: Double): Double {
        return Math.min(max, Math.max(min, value))
    }

    // ============================================================
    //  HDR 像素变换
    // ============================================================

    /**
     * 对原始 RGBA 像素数据执行 HDR 变换
     *
     * @param pixels 输入 RGBA 像素数组 (每像素 4 字节, 0-255)
     * @param width  图片宽度
     * @param height 图片高度
     * @param settings 转换参数
     * @return 变换后的 RGBA 像素数组 (每像素 4 字节, 0-255)
     */
    fun applyHdrTransform(
        pixels: ByteArray,
        width: Int,
        height: Int,
        settings: ConversionSettings
    ): ByteArray {
        // CUDA 加速：GPU 不可用/失败时自动回退 CPU
        if (HdrGpuJni.isAvailable) {
            try {
                val out = ByteArray(pixels.size)
                if (HdrGpuJni.nativeApplyHdrTransform(
                        pixels, width, height,
                        settings.totalExposure, settings.gamma,
                        settings.rAdj, settings.gAdj, settings.bAdj, out
                    )
                ) {
                    return out
                }
            } catch (e: Throwable) {
                System.err.println("[HdrGpuJni] applyHdrTransform GPU 失败，回退 CPU: ${e.message}")
            }
        }

        val totalPixels = width * height
        val linear = DoubleArray(totalPixels * 3)
        var sum = 0.0

        // Pass 1: sRGB → 线性 + 计算平均亮度
        for (i in 0 until totalPixels) {
            val base = i * 4
            val r = srgbToLinear((pixels[base].toInt() and 0xFF) / 255.0)
            val g = srgbToLinear((pixels[base + 1].toInt() and 0xFF) / 255.0)
            val b = srgbToLinear((pixels[base + 2].toInt() and 0xFF) / 255.0)
            val offset = i * 3
            linear[offset] = r
            linear[offset + 1] = g
            linear[offset + 2] = b
            sum += 0.2126 * r + 0.7152 * g + 0.0722 * b
        }

        // Pass 2: 自动伽马（基于平均亮度自适应调整）
        val mean = sum / totalPixels
        if (mean > 0.001 && mean < 0.999) {
            val autoGamma = Math.log(0.5) / Math.log(mean)
            val clampedGamma = clamp(autoGamma, 0.3, 3.0)
            for (i in linear.indices) {
                linear[i] = Math.pow(clamp(linear[i], 0.0, Double.MAX_VALUE), clampedGamma)
            }
        }

        // Pass 3: RGB 通道调整 + 曝光 + 伽马 + sRGB 编码
        val totalExposure = settings.totalExposure
        val rAdj = settings.rAdj
        val gAdj = settings.gAdj
        val bAdj = settings.bAdj
        val gamma = settings.gamma

        val output = ByteArray(totalPixels * 4)

        for (i in 0 until totalPixels) {
            val offset = i * 3
            var r = linear[offset]
            var g = linear[offset + 1]
            var b = linear[offset + 2]

            r *= rAdj
            g *= gAdj
            b *= bAdj

            r *= totalExposure
            g *= totalExposure
            b *= totalExposure

            r = Math.pow(clamp(r, 0.0, Double.MAX_VALUE), gamma)
            g = Math.pow(clamp(g, 0.0, Double.MAX_VALUE), gamma)
            b = Math.pow(clamp(b, 0.0, Double.MAX_VALUE), gamma)

            val sr = clamp(linearToSrgb(r), 0.0, 1.0)
            val sg = clamp(linearToSrgb(g), 0.0, 1.0)
            val sb = clamp(linearToSrgb(b), 0.0, 1.0)

            val outIndex = i * 4
            output[outIndex] = kotlin.math.round(sr * 255).toInt().coerceIn(0, 255).toByte()
            output[outIndex + 1] = kotlin.math.round(sg * 255).toInt().coerceIn(0, 255).toByte()
            output[outIndex + 2] = kotlin.math.round(sb * 255).toInt().coerceIn(0, 255).toByte()
            output[outIndex + 3] = 255.toByte()
        }

        return output
    }

    /**
     * 图片 HDR 变换 → Rec.2020/PQ 编码（2026-08-13，与视频直接预览同构的色彩管线）。
     * 变换语义与 applyHdrTransform 完全一致（自动伽马 + 曝光 + RGB 通道 + 伽马），
     * 但末尾不再 linearToSrgb，而是 Rec.709→Rec.2020→PQ 编码，
     * 使像素与注入的 Rec.2020/PQ ICC 一致（否则 sRGB 数值被当 Rec.2020/PQ 解释 → 色彩错乱）。
     */
    fun applyHdrTransformToRec2020Pq(
        pixels: ByteArray,
        width: Int,
        height: Int,
        settings: ConversionSettings,
        whiteNits: Double
    ): ByteArray {
        // 曝光 = 峰值/白点 = 2^EV（与视频预览一致；微调明暗已移除，不乘它避免压暗 HDR/发灰）
        val exposure = (settings.peakNits ?: 1000.0) / whiteNits
        // CUDA 加速：GPU 不可用/失败时自动回退 CPU
        if (HdrGpuJni.isAvailable) {
            try {
                val out = ByteArray(pixels.size)
                if (HdrGpuJni.nativeApplyHdrTransformToRec2020Pq(
                        pixels, width, height,
                        exposure, settings.gamma,
                        settings.rAdj, settings.gAdj, settings.bAdj,
                        whiteNits, out
                    )
                ) {
                    return out
                }
            } catch (e: Throwable) {
                System.err.println("[HdrGpuJni] applyHdrTransformToRec2020Pq GPU 失败，回退 CPU: ${e.message}")
            }
        }
        val totalPixels = width * height
        val linear = DoubleArray(totalPixels * 3)
        // Pass 1: sRGB → 线性（不做自动伽马：与视频预览一致，避免偏暗/亮图被整体提亮导致发白）
        for (i in 0 until totalPixels) {
            val base = i * 4
            val r = srgbToLinear((pixels[base].toInt() and 0xFF) / 255.0)
            val g = srgbToLinear((pixels[base + 1].toInt() and 0xFF) / 255.0)
            val b = srgbToLinear((pixels[base + 2].toInt() and 0xFF) / 255.0)
            val offset = i * 3
            linear[offset] = r
            linear[offset + 1] = g
            linear[offset + 2] = b
        }
        // Pass 3: RGB 通道 + 曝光 + 伽马 → 线性(Rec.709) → Rec.2020 → PQ
        val rAdj = settings.rAdj
        val gAdj = settings.gAdj
        val bAdj = settings.bAdj
        val gamma = settings.gamma
        val scale = whiteNits / 10000.0
        val out = ByteArray(totalPixels * 4)
        for (i in 0 until totalPixels) {
            val offset = i * 3
            var r = linear[offset] * rAdj * exposure
            var g = linear[offset + 1] * gAdj * exposure
            var b = linear[offset + 2] * bAdj * exposure
            r = Math.pow(Math.max(r, 0.0), gamma)
            g = Math.pow(Math.max(g, 0.0), gamma)
            b = Math.pow(Math.max(b, 0.0), gamma)
            // Rec.709 → Rec.2020（与视频 zscale pin=bt709 → p=bt2020 一致）
            val r2020 = 0.6274038959 * r + 0.3292830384 * g + 0.0433130642 * b
            val g2020 = 0.0690972894 * r + 0.9195403951 * g + 0.0113623156 * b
            val b2020 = 0.0163914389 * r + 0.0880133078 * g + 0.8955952528 * b
            val o = i * 4
            out[o] = Math.round(pqEncode(r2020 * scale) * 255).toInt().coerceIn(0, 255).toByte()
            out[o + 1] = Math.round(pqEncode(g2020 * scale) * 255).toInt().coerceIn(0, 255).toByte()
            out[o + 2] = Math.round(pqEncode(b2020 * scale) * 255).toInt().coerceIn(0, 255).toByte()
            out[o + 3] = 255.toByte()
        }
        return out
    }

    /** PQ 编码（线性 L∈[0,1] 相对 10000 尼特 → PQ 码 0..1，SMPTE ST 2084） */
    private fun pqEncode(l: Double): Double {
        val ll = clamp(l, 0.0, 1.0)
        val m1 = 0.1593017578125
        val m2 = 78.84375
        val c1 = 0.8359375
        val c2 = 18.8515625
        val c3 = 18.6875
        val lm1 = Math.pow(ll, m1)
        return Math.pow((c1 + c2 * lm1) / (1.0 + c3 * lm1), m2)
    }

    // ============================================================
    //  图片读取 / 写入
    // ============================================================

    /**
     * 读取图片为 RGBA 像素数组
     */
    fun readImageAsRgba(inputPath: String): ImageData {
        val raw = ImageIO.read(File(inputPath))
            ?: throw IllegalArgumentException("无法读取图片: $inputPath")
        // 按 EXIF Orientation 转正：解码出的 BufferedImage 像素是「原样未旋转」的存储数据，
        // 应用 orientation 把图片转成正面，最终输出（预览 / 转换文件 / 视频帧）即与显示器一致。
        //   orientation 1 = 原图（不动）
        //   2 = 水平镜像 3 = 旋转 180  4 = 垂直镜像
        //   5 = 转置     6 = 旋转 90 CW (顺时针)  7 = 横向翻转 (= anti-diagonal)  8 = 旋转 90 CCW
        val img = applyOrientation(raw, exifOrientation(inputPath))

        val width = img.width
        val height = img.height

        // 转为 RGBA 像素数组
        val pixels = ByteArray(width * height * 4)
        val argbArray = IntArray(width * height)
        img.getRGB(0, 0, width, height, argbArray, 0, width)

        for (i in 0 until width * height) {
            val argb = argbArray[i]
            pixels[i * 4] = ((argb shr 16) and 0xFF).toByte()      // R
            pixels[i * 4 + 1] = ((argb shr 8) and 0xFF).toByte()   // G
            pixels[i * 4 + 2] = (argb and 0xFF).toByte()            // B
            pixels[i * 4 + 3] = 255.toByte()                        // A
        }

        return ImageData(pixels, width, height)
    }

    /**
     * 将 RGBA 像素数据写入 BufferedImage
     */
    fun pixelsToBufferedImage(pixels: ByteArray, width: Int, height: Int): BufferedImage {
        val img = BufferedImage(width, height, BufferedImage.TYPE_INT_RGB)
        val argbArray = IntArray(width * height)

        for (i in 0 until width * height) {
            val r = pixels[i * 4].toInt() and 0xFF
            val g = pixels[i * 4 + 1].toInt() and 0xFF
            val b = pixels[i * 4 + 2].toInt() and 0xFF
            argbArray[i] = (0xFF shl 24) or (r shl 16) or (g shl 8) or b
        }

        img.setRGB(0, 0, width, height, argbArray, 0, width)
        return img
    }

    /**
     * 将 RGBA 像素数据编码为 JPEG（指定质量 0..1）
     */
    fun encodeJpeg(rgba: ByteArray, width: Int, height: Int, quality: Float): ByteArray {
        val img = pixelsToBufferedImage(rgba, width, height)
        val baos = ByteArrayOutputStream()
        val writer = ImageIO.getImageWritersByFormatName("jpg").next()
        val param = writer.defaultWriteParam
        param.compressionMode = ImageWriteParam.MODE_EXPLICIT
        param.compressionQuality = quality.coerceIn(0.1f, 1.0f)
        writer.output = MemoryCacheImageOutputStream(baos)
        writer.write(null, IIOImage(img, null, null), param)
        writer.dispose()
        return baos.toByteArray()
    }

    /**
     * 读取图片并缩放到预览尺寸（默认缩放到原图的 50%）
     * 先按 EXIF orientation 转正再缩放，保证预览输出（像素与尺寸）与浏览器显示的 SDR 原图一致。
     */
    fun readImageForPreview(inputPath: String, scaleRatio: Double = 0.5): ImageData {
        val raw = ImageIO.read(File(inputPath))
            ?: throw IllegalArgumentException("无法读取图片: $inputPath")
        val img = applyOrientation(raw, exifOrientation(inputPath))

        var w = (img.width * scaleRatio).toInt().coerceAtLeast(1)
        var h = (img.height * scaleRatio).toInt().coerceAtLeast(1)

        // 使用 Java 2D 缩放
        val scaled = BufferedImage(w, h, BufferedImage.TYPE_INT_RGB)
        val g2d = scaled.createGraphics()
        g2d.drawImage(img, 0, 0, w, h, null)
        g2d.dispose()

        val pixels = ByteArray(w * h * 4)
        val argbArray = IntArray(w * h)
        scaled.getRGB(0, 0, w, h, argbArray, 0, w)

        for (i in 0 until w * h) {
            val argb = argbArray[i]
            pixels[i * 4] = ((argb shr 16) and 0xFF).toByte()
            pixels[i * 4 + 1] = ((argb shr 8) and 0xFF).toByte()
            pixels[i * 4 + 2] = (argb and 0xFF).toByte()
            pixels[i * 4 + 3] = 255.toByte()
        }

        return ImageData(pixels, w, h)
    }

    // ============================================================
    //  EXIF Orientation 处理（让输出窗口的预览/导出与原图侧面一致地「转正」）
    // ============================================================

    /**
     * 读取 JPEG/EXIF Orientation（1..8）。非 JPEG（含 PNG/WebP）一律返回 1。
     * 视频链路传入的 PNG 帧无 EXIF，本函数对 PNG 第一字节即返回（读取几乎为空）。
     * 仿照 ImageIO 不支持 orientation 的事实，本后端基于 PIL/TwelveMonkeys 等常见实现的成熟做法，
     * 自行扫描 APP1/Exif/TIFF 段，避免引入额外依赖。
     */
    fun exifOrientation(inputPath: String): Int {
        val f = File(inputPath)
        if (!f.exists() || f.length() <= 4 || f.length() > 64L * 1024 * 1024) return 1
        return try {
            val data = f.readBytes()
            parseJpegExifOrientation(data)
        } catch (e: Throwable) {
            1
        }
    }

    private fun parseJpegExifOrientation(data: ByteArray): Int {
        if (data.size < 4) return 1
        // JPEG magic: FF D8 FF
        if (data[0].toInt() and 0xFF != 0xFF) return 1
        if (data[1].toInt() and 0xFF != 0xD8) return 1
        if (data[2].toInt() and 0xFF != 0xFF) return 1
        var i = 2
        while (i + 3 < data.size) {
            if (data[i].toInt() and 0xFF != 0xFF) { i++; continue }
            val marker = data[i + 1].toInt() and 0xFF
            // 填充字节或 SOI：跳过
            if (marker == 0xFF || marker == 0xD8) { i += 2; continue }
            // EOI / SOS：之后不再有 APP1（EXIF 始终在 SOI 之后、SOS 之前；这里最多跳过已无关）
            if (marker == 0xD9 || marker == 0xDA) return 1
            // standalone markers (RST0..RST7 / TEM)
            if (marker == 0x01 || marker in 0xD0..0xD7) { i += 2; continue }
            // 一般 segment：长度 2 字节（大端，含自身）
            if (i + 4 > data.size) return 1
            val len = ((data[i + 2].toInt() and 0xFF) shl 8) or (data[i + 3].toInt() and 0xFF)
            if (len < 2 || i + 2 + len > data.size) return 1
            if (marker == 0xE1) {
                // APP1：检查 "Exif\0\0" 前缀（6 字节）
                val exifStart = i + 4
                if (len >= 10 &&
                    data[exifStart].toInt() == 0x45 /*E*/ &&
                    data[exifStart + 1].toInt() == 0x78 /*x*/ &&
                    data[exifStart + 2].toInt() == 0x69 /*i*/ &&
                    data[exifStart + 3].toInt() == 0x66 /*f*/ &&
                    data[exifStart + 4].toInt() == 0x00 &&
                    data[exifStart + 5].toInt() == 0x00
                ) {
                    val tiffStart = exifStart + 6
                    return parseTiffOrientation(data, tiffStart)
                }
                // APP1 不是 EXIF（可能是 XMP）；继续后续段
            }
            i += 2 + len
        }
        return 1
    }

    /**
     * 解析 TIFF 头 + IFD0，找 Orientation (tag 0x0112, SHORT)。找不到或异常返回 1。
     */
    private fun parseTiffOrientation(data: ByteArray, start: Int): Int {
        if (start + 8 > data.size) return 1
        val b0 = data[start].toInt() and 0xFF
        val b1 = data[start + 1].toInt() and 0xFF
        val le = b0 == 0x49 /*I*/ && b1 == 0x49 /*I*/
        val be = b0 == 0x4D /*M*/ && b1 == 0x4D /*M*/
        if (!le && !be) return 1

        fun u16(o: Int): Int =
            if (le) (data[o].toInt() and 0xFF) or ((data[o + 1].toInt() and 0xFF) shl 8)
            else ((data[o].toInt() and 0xFF) shl 8) or (data[o + 1].toInt() and 0xFF)

        fun u32(o: Int): Int =
            if (le) (data[o].toInt() and 0xFF) or
                    ((data[o + 1].toInt() and 0xFF) shl 8) or
                    ((data[o + 2].toInt() and 0xFF) shl 16) or
                    ((data[o + 3].toInt() and 0xFF) shl 24)
            else ((data[o].toInt() and 0xFF) shl 24) or
                 ((data[o + 1].toInt() and 0xFF) shl 16) or
                 ((data[o + 2].toInt() and 0xFF) shl 8) or
                 (data[o + 3].toInt() and 0xFF)

        if (u16(start + 2) != 42) return 1
        val ifdStart = start + u32(start + 4)
        if (ifdStart + 2 > data.size) return 1
        val n = u16(ifdStart)
        if (ifdStart + 2L + n.toLong() * 12L > data.size) return 1
        for (j in 0 until n) {
            val e = ifdStart + 2 + j * 12
            if (u16(e) == 0x0112) {
                // Orientation 存储为 SHORT，count=1 时数值直接在 e+8 .. e+9 的 2 字节内
                val v = u16(e + 8)
                return if (v in 1..8) v else 1
            }
        }
        return 1
    }

    /**
     * 根据 EXIF Orientation 把存储像素转成「正向显示像素」。
     *   orientation 1: 原图
     *   2: 水平翻转（无旋转）             3: 旋转 180
     *   4: 垂直翻转（无旋转）             5: 转置（横竖对换+镜像）
     *   6: 旋转 90 CW（横竖对换，length=width）
     *   7: 横向翻转（反对角线镜像）       8: 旋转 90 CCW（横竖对换）
     *
     * 返回新 BufferedImage（5..8 交换宽高），若 orientation <= 1 直接返回原图避免拷贝。
     * 几何参数使用业界通用的 AffineTransform 6 元写法（[m00,m10,m01,m11,m02,m12]），
     *   与 TwelveMonkeys / Apache Commons Imaging / lyndon 指明的同一套定义一致。
     */
    fun applyOrientation(img: BufferedImage, orientation: Int): BufferedImage {
        if (orientation <= 1) return img
        if (orientation > 8) return img
        val w = img.width
        val h = img.height
        if (w < 1 || h < 1) return img

        val swap = orientation in 5..8
        val outW = if (swap) h else w
        val outH = if (swap) w else h
        val type = if (img.colorModel != null && img.colorModel.hasAlpha())
            BufferedImage.TYPE_INT_ARGB else BufferedImage.TYPE_INT_RGB
        val out = BufferedImage(outW, outH, type)
        val g = out.createGraphics()
        try {
            g.setRenderingHint(RenderingHints.KEY_INTERPOLATION, RenderingHints.VALUE_INTERPOLATION_BILINEAR)
            g.setRenderingHint(RenderingHints.KEY_RENDERING, RenderingHints.VALUE_RENDER_QUALITY)
            // 标准 Java AffineTransform 6 元构造：AffineTransform(m00, m10, m01, m11, m02, m12)
            //   x' = m00*x + m01*y + m02
            //   y' = m10*x + m11*y + m12
            // Graphics2D.drawImage(src, at, null) 将源图像以 at 映射到目标坐标。
            // 以下六组参数对应 orientation 2..8 的存储 -> 显示变换（取自 TwelveMonkeys / 多家公开参考）。
            val at: AffineTransform = when (orientation) {
                2 -> AffineTransform(-1.0, 0.0, 0.0, 1.0, w.toDouble(), 0.0)        // mirror X
                3 -> AffineTransform(-1.0, 0.0, 0.0, -1.0, w.toDouble(), h.toDouble()) // rotate 180
                4 -> AffineTransform(1.0, 0.0, 0.0, -1.0, 0.0, h.toDouble())        // mirror Y
                5 -> AffineTransform(0.0, 1.0, 1.0, 0.0, 0.0, 0.0)                  // transpose
                6 -> AffineTransform(0.0, 1.0, -1.0, 0.0, h.toDouble(), 0.0)         // rotate 90 CW (swap dims)
                7 -> AffineTransform(0.0, -1.0, -1.0, 0.0, h.toDouble(), w.toDouble()) // transverse
                8 -> AffineTransform(0.0, -1.0, 1.0, 0.0, 0.0, w.toDouble())         // rotate 90 CCW (swap dims)
                else -> AffineTransform(1.0, 0.0, 0.0, 1.0, 0.0, 0.0)
            }
            g.drawImage(img, at, null)
        } finally {
            g.dispose()
        }
        return out
    }

    data class ImageData(
        val pixels: ByteArray,
        val width: Int,
        val height: Int
    )
}
