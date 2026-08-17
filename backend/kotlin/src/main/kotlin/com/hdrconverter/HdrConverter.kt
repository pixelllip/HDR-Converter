package com.hdrconverter

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
        val img = ImageIO.read(File(inputPath))
            ?: throw IllegalArgumentException("无法读取图片: $inputPath")

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
     */
    fun readImageForPreview(inputPath: String, scaleRatio: Double = 0.5): ImageData {
        val img = ImageIO.read(File(inputPath))
            ?: throw IllegalArgumentException("无法读取图片: $inputPath")

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

    data class ImageData(
        val pixels: ByteArray,
        val width: Int,
        val height: Int
    )
}
