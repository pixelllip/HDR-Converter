package com.hdrconverter

import java.io.File
import java.io.FileInputStream

/**
 * 输入图像色彩空间检测
 *
 * 依据 Ultra HDR 规范："SDR 图像的色彩配置定义了 HDR 图像的色彩空间"，
 * 程序不应假设输入是 sRGB，而应先检测原图的实际色彩空间（ICC / EXIF / JFIF / PNG 标记），
 * 再据此决定主图与增益图的色彩空间处理。
 *
 * 检测顺序：ICC(APP2/iCCP) > EXIF ColorSpace > JFIF/PNG 标记 > UNKNOWN(默认 sRGB 假设)
 */
enum class InputColorSpace(val displayName: String) {
    SRGB("sRGB (IEC 61966-2-1)"),
    DISPLAY_P3("Display-P3 (D65)"),
    ADOBE_RGB("Adobe RGB (1998)"),
    UNKNOWN("未声明（按 sRGB 假设）")
}

object ColorSpaceDetector {

    // 已知基色（线性 XYZ，ICC s15Fixed16 归一化，rXYZ 标签值）
    // 参考 Rec.709/sRGB、Display-P3、AdobeRGB
    private val SRGB_R = doubleArrayOf(0.436041, 0.222485, 0.013920)
    private val P3_R = doubleArrayOf(0.515102, 0.241186, -0.001126)
    private val ADOBE_R = doubleArrayOf(0.609740, 0.205940, 0.149190)
    private val SRGB_G = doubleArrayOf(0.385113, 0.716879, 0.097109)
    private val P3_G = doubleArrayOf(0.291979, 0.692219, 0.041882)
    private val ADOBE_G = doubleArrayOf(0.311110, 0.625710, 0.063250)
    private val SRGB_B = doubleArrayOf(0.143051, 0.060608, 0.713913)
    private val P3_B = doubleArrayOf(0.157101, 0.066593, 0.784072)
    private val ADOBE_B = doubleArrayOf(0.125710, 0.070240, 0.991050)

    /** 读取文件头部若干字节用于元数据解析（色彩段都在头部） */
    private fun readHead(path: String, max: Int = 256 * 1024): ByteArray {
        FileInputStream(File(path)).use { `in` ->
            val buf = ByteArray(max)
            val n = `in`.read(buf)
            return if (n > 0) buf.copyOf(n) else ByteArray(0)
        }
    }

    private fun u16(b: ByteArray, o: Int): Int =
        ((b[o].toInt() and 0xFF) shl 8) or (b[o + 1].toInt() and 0xFF)

    private fun u32(b: ByteArray, o: Int): Long =
        ((b[o].toLong() and 0xFF) shl 24) or ((b[o + 1].toLong() and 0xFF) shl 16) or
                ((b[o + 2].toLong() and 0xFF) shl 8) or (b[o + 3].toLong() and 0xFF)

    private fun s15fixed(b: ByteArray, o: Int): Double = u32(b, o).toInt() / 65536.0

    /** 基色匹配：比较三个主色（忽略微小差异），返回最接近的已知空间 */
    private fun matchPrimaries(r: DoubleArray, g: DoubleArray, b: DoubleArray): InputColorSpace? {
        fun dist(a: DoubleArray, ref: DoubleArray): Double {
            var d = 0.0
            for (i in 0..2) d += (a[i] - ref[i]) * (a[i] - ref[i])
            return Math.sqrt(d)
        }
        val sets = listOf(
            Triple(SRGB_R, SRGB_G, SRGB_B) to InputColorSpace.SRGB,
            Triple(P3_R, P3_G, P3_B) to InputColorSpace.DISPLAY_P3,
            Triple(ADOBE_R, ADOBE_G, ADOBE_B) to InputColorSpace.ADOBE_RGB
        )
        var best: InputColorSpace? = null
        var bestD = 1e9
        for ((ref, kind) in sets) {
            val d = dist(r, ref.first) + dist(g, ref.second) + dist(b, ref.third)
            if (d < bestD) { bestD = d; best = kind }
        }
        return if (bestD < 0.02) best else null // 阈值：主色差异足够小才判定
    }

    /** 解析 ICC 的 RGB 基色标签（rXYZ/gXYZ/bXYZ），返回匹配的色彩空间 */
    private fun iccToColorSpace(icc: ByteArray): InputColorSpace? {
        if (icc.size < 132) return null
        if (icc[36] != 'a'.code.toByte() || icc[37] != 'c'.code.toByte() ||
            icc[38] != 's'.code.toByte() || icc[39] != 'p'.code.toByte()) return null
        // tag table
        val tagCount = u32(icc, 128).toInt()
        if (tagCount <= 0 || tagCount > 64) return null
        var r: DoubleArray? = null
        var g: DoubleArray? = null
        var b: DoubleArray? = null
        for (i in 0 until tagCount) {
            val base = 132 + i * 12
            if (base + 12 > icc.size) break
            val sig = String(icc, base, 4, Charsets.US_ASCII)
            val off = u32(icc, base + 4).toInt()
            if (off < 0 || off + 20 > icc.size) continue
            val x = s15fixed(icc, off + 8)
            val y = s15fixed(icc, off + 12)
            val z = s15fixed(icc, off + 16)
            when (sig) {
                "rXYZ" -> r = doubleArrayOf(x, y, z)
                "gXYZ" -> g = doubleArrayOf(x, y, z)
                "bXYZ" -> b = doubleArrayOf(x, y, z)
            }
        }
        if (r == null || g == null || b == null) return null
        return matchPrimaries(r, g, b)
    }

    /** 从 JPEG 字节检测色彩空间 */
    private fun detectJpeg(b: ByteArray): InputColorSpace {
        var off = 2
        var hasJfif = false
        var iccMatch: InputColorSpace? = null
        var exifColorSpace: InputColorSpace? = null
        while (off + 4 <= b.size) {
            if ((b[off].toInt() and 0xFF) != 0xFF) break
            val marker = u16(b, off)
            if (marker == 0xFFDA || marker == 0xFFD9) break
            val len = u16(b, off + 2)
            if (len < 2 || off + 2 + len > b.size) break
            val data = b.sliceArray(off + 4 until off + 2 + len)
            when (marker) {
                0xFFE0 -> {
                    // JFIF：无色彩声明，惯例 sRGB（不强制）
                    if (data.size >= 5 && data[0] == 'J'.code.toByte() && data[1] == 'F'.code.toByte()) hasJfif = true
                }
                0xFFE1 -> {
                    // EXIF
                    if (data.size >= 6 && data[0] == 'E'.code.toByte() && data[1] == 'x'.code.toByte() &&
                        data[2] == 'i'.code.toByte() && data[3] == 'f'.code.toByte()
                    ) {
                        exifColorSpace = parseExifColorSpace(data)
                    }
                }
                0xFFE2 -> {
                    // ICC_PROFILE：段 payload = "ICC_PROFILE\0"(12) + seq_no(1) + total(1) + ICC 数据
                    if (data.size >= 14 &&
                        data.sliceArray(0 until 12).toString(Charsets.US_ASCII) == "ICC_PROFILE\u0000"
                    ) {
                        val icc = data.copyOfRange(14, data.size)
                        iccMatch = iccToColorSpace(icc)
                    }
                }
            }
            off += 2 + len
        }
        // 优先级：ICC > EXIF > JFIF(sRGB)
        iccMatch?.let { return it }
        exifColorSpace?.let { return it }
        if (hasJfif) return InputColorSpace.SRGB
        return InputColorSpace.UNKNOWN
    }

    /** 解析 EXIF 的 ColorSpace tag (0xA001)：1=sRGB, 2=AdobeRGB */
    private fun parseExifColorSpace(exif: ByteArray): InputColorSpace? {
        // 跳过 "Exif\0\0"
        var t = 6
        // TIFF 头：II/MM + 0x2A + IFD0 offset
        if (t + 8 > exif.size) return null
        val tiff = t
        val mm = (exif[t].toInt() and 0xFF).toChar() == 'M' && (exif[t + 1].toInt() and 0xFF).toChar() == 'M'
        val ii = (exif[t].toInt() and 0xFF).toChar() == 'I' && (exif[t + 1].toInt() and 0xFF).toChar() == 'I'
        if (!mm && !ii) return null
        val ifd0 = if (mm) u32(exif, t + 4).toInt() else {
            ((exif[t + 4].toInt() and 0xFF) or ((exif[t + 5].toInt() and 0xFF) shl 8) or
                    ((exif[t + 6].toInt() and 0xFF) shl 16) or ((exif[t + 7].toInt() and 0xFF) shl 24))
        }
        val p = tiff + ifd0
        if (p + 2 > exif.size) return null
        val nTags = if (mm) u16(exif, p) else {
            (exif[p].toInt() and 0xFF) or ((exif[p + 1].toInt() and 0xFF) shl 8)
        }
        for (i in 0 until nTags) {
            val tp = p + 2 + i * 12
            if (tp + 12 > exif.size) break
            val tag = if (mm) u16(exif, tp) else {
                (exif[tp].toInt() and 0xFF) or ((exif[tp + 1].toInt() and 0xFF) shl 8)
            }
            if (tag == 0xA001) {
                // 值在 offset 8（4 字节内联）
                val v = if (mm) u32(exif, tp + 8) else {
                    ((exif[tp + 8].toLong() and 0xFF) or ((exif[tp + 9].toLong() and 0xFF) shl 8) or
                            ((exif[tp + 10].toLong() and 0xFF) shl 16) or ((exif[tp + 11].toLong() and 0xFF) shl 24))
                }
                return when (v) {
                    1L -> InputColorSpace.SRGB
                    2L -> InputColorSpace.ADOBE_RGB
                    else -> null
                }
            }
        }
        return null
    }

    /** 从 PNG 字节检测色彩空间 */
    private fun detectPng(b: ByteArray): InputColorSpace {
        var p = 8
        while (p + 8 <= b.size) {
            val len = ((b[p].toInt() and 0xFF) shl 24) or ((b[p + 1].toInt() and 0xFF) shl 16) or
                    ((b[p + 2].toInt() and 0xFF) shl 8) or (b[p + 3].toInt() and 0xFF)
            val type = String(b, p + 4, 4, Charsets.US_ASCII)
            if (type == "IEND") break
            if (type == "sRGB") return InputColorSpace.SRGB
            if (type == "gAMA") return InputColorSpace.SRGB // gamma 近似，多数 sRGB
            // iCCP：解析 profile name 后跳 profile data（压缩），只检测是否存在
            if (type == "iCCP") {
                // 简化：iCCP 数据含压缩 ICC，头部 name + compression byte；完整解析较复杂，标记 unknown 并依赖 ICC 检测不可用
                return InputColorSpace.UNKNOWN
            }
            p += 12 + len
        }
        return InputColorSpace.UNKNOWN
    }

    /**
     * 检测输入图像色彩空间
     * @return InputColorSpace
     */
    fun detect(inputPath: String): InputColorSpace {
        val b = readHead(inputPath)
        if (b.size < 8) return InputColorSpace.UNKNOWN
        // PNG 签名：89 50 4E 47
        if ((b[0].toInt() and 0xFF) == 0x89 && b[1] == 'P'.code.toByte() && b[2] == 'N'.code.toByte() && b[3] == 'G'.code.toByte()) {
            return detectPng(b)
        }
        // JPEG 签名：FF D8
        if ((b[0].toInt() and 0xFF) == 0xFF && (b[1].toInt() and 0xFF) == 0xD8) {
            return detectJpeg(b)
        }
        return InputColorSpace.UNKNOWN
    }
}
