package com.hdrconverter

import java.awt.image.BufferedImage
import java.io.ByteArrayOutputStream
import javax.imageio.IIOImage
import javax.imageio.ImageIO
import javax.imageio.ImageWriteParam
import javax.imageio.stream.MemoryCacheImageOutputStream

/**
 * Ultra HDR JPEG 编码器（Kotlin 版，与 JS 端 backend/ultra_hdr.js 保持一致）
 *
 * 符合 Android "Ultra HDR Image Format" v1.0 / v1.1：
 *   - 主图像为 SDR 渲染底图（默认 Display-P3 像素 + P3 ICC；primarySrgb=true 时保持 sRGB + sRGB ICC）
 *   - 增益图为灰度 JPEG，编码对数空间的 recovery 值
 *   - 主图像 XMP 写入 GContainer + hdrgm:Version
 *   - 增益图 XMP 写入 hdrgm 增益图元数据
 *   - MPF（APP2 "MPF\0"）多图索引，位于主图像 SOS 之前
 *
 * 文件布局（与 libultrahdr 一致）:
 *   [主图像] SOI + APP0 + APP1(XMP GContainer) + APP2(ICC) + DQT/SOF/DHT
 *            + APP2(MPF) + SOS + 数据 + EOI
 *   [增益图] SOI + APP1(XMP hdrgm) + DQT/SOF/DHT + SOS + 数据 + EOI
 */
object UltraHdrEncoder {

    // ============================================================
    //  ICC 配置文件（取自真实 Google Ultra HDR 文件，与 libultrahdr 输出一致）
    // ============================================================

    /** 主图像 ICC：Display-P3 基色 + sRGB 传递函数（Google Inc. 2016） */
    private val UHDR_PRIMARY_ICC: ByteArray = java.util.Base64.getDecoder().decode(
        "AAACCAAAAAAEMAAAbW50clJHQiBYWVogB+AAAQABAAAAAAAAYWNzcAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAEAAPbWAAEAAAAA0y0AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAJZGVzYwAAAPAAAABkclhZWgAAAVQAAAAUZ1hZWgAAAWgAAAAUYlhZWgAAAXwAAAAUd3RwdAAAAZAAAAAUclRSQwAAAaQAAAAoZ1RSQwAAAaQAAAAoYlRSQwAAAaQAAAAoY3BydAAAAcwAAAA8bWx1YwAAAAAAAAABAAAADGVuVVMAAABGAAAAHABEAGkAcwBwAGwAYQB5ACAAUAAzACAARwBhAG0AdQB0ACAAdwBpAHQAaAAgAHMAUgBHAEIAIABUAHIAYQBuAHMAZgBlAHIAAFhZWiAAAAAAAACD3wAAPb////+7WFlaIAAAAAAAAEq/AACxNwAACrlYWVogAAAAAAAAKDgAABELAADIuVhZWiAAAAAAAAD21gABAAAAANMtcGFyYQAAAAAABAAAAAJmZgAA8qcAAA1ZAAAT0AAAClsAAAAAAAAAAG1sdWMAAAAAAAAAAQAAAAxlblVTAAAAIAAAABwARwBvAG8AZwBsAGUAIABJAG4AYwAuACAAMgAwADEANg=="
    )

    /** 增益图 ICC：sRGB（Google Inc. 2016） */
    private val UHDR_GAINMAP_ICC: ByteArray = java.util.Base64.getDecoder().decode(
        "AAAByAAAAAAEMAAAbW50clJHQiBYWVogB+AAAQABAAAAAAAAYWNzcAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAEAAPbWAAEAAAAA0y0AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAJZGVzYwAAAPAAAAAkclhZWgAAARQAAAAUZ1hZWgAAASgAAAAUYlhZWgAAATwAAAAUd3RwdAAAAVAAAAAUclRSQwAAAWQAAAAoZ1RSQwAAAWQAAAAoYlRSQwAAAWQAAAAoY3BydAAAAYwAAAA8bWx1YwAAAAAAAAABAAAADGVuVVMAAAAIAAAAHABzAFIARwBCWFlaIAAAAAAAAG+iAAA49QAAA5BYWVogAAAAAAAAYpkAALeFAAAY2lhZWiAAAAAAAAAkoAAAD4QAALbPWFlaIAAAAAAAAPbWAAEAAAAA0y1wYXJhAAAAAAAEAAAAAmZmAADypwAADVkAABPQAAAKWwAAAAAAAAAAbWx1YwAAAAAAAAABAAAADGVuVVMAAAAgAAAAHABHAG8AbwBnAGwAZQAgAEkAbgBjAC4AIAAyADAAMQA2"
    )

    // ============================================================
    //  基础工具
    // ============================================================

    private fun srgbToLinear(v: Double): Double =
        if (v <= 0.04045) v / 12.92 else Math.pow((v + 0.055) / 1.055, 2.4)

    private fun linearToSrgb(v: Double): Double =
        if (v <= 0.0031308) v * 12.92 else 1.055 * Math.pow(v, 1.0 / 2.4) - 0.055

    /**
     * 将 sRGB 编码的 RGBA 转换为 Display-P3（sRGB 传递函数）编码的 RGBA
     * 主图像 ICC 为 Display-P3 基色 + sRGB 传递，像素必须与其一致，否则浏览器色彩管理会整体偏移（泛白）
     * 优先使用 CUDA 加速，GPU 不可用时回退 CPU。
     */
    private fun srgbRgbaToDisplayP3Rgba(rgba: ByteArray, width: Int, height: Int): ByteArray {
        if (HdrGpuJni.isAvailable) {
            try {
                val out = ByteArray(rgba.size)
                if (HdrGpuJni.nativeSrgbToDisplayP3(rgba, width, height, out)) {
                    return out
                }
            } catch (e: Throwable) {
                System.err.println("[HdrGpuJni] srgbToDisplayP3 GPU 失败，回退 CPU: ${e.message}")
            }
        }
        val out = ByteArray(rgba.size)
        for (i in 0 until rgba.size step 4) {
            val r = srgbToLinear((rgba[i].toInt() and 0xFF) / 255.0)
            val g = srgbToLinear((rgba[i + 1].toInt() and 0xFF) / 255.0)
            val b = srgbToLinear((rgba[i + 2].toInt() and 0xFF) / 255.0)
            // sRGB 线性 -> Display-P3 线性（D65 到 D65）
            val pr = 0.82246 * r + 0.17749 * g + 0.00005 * b
            val pg = 0.03311 * r + 0.96687 * g + 0.00002 * b
            val pb = 0.01709 * r + 0.07239 * g + 0.91053 * b
            out[i] = Math.round(linearToSrgb(clamp(pr, 0.0, 1.0)) * 255).toInt().coerceIn(0, 255).toByte()
            out[i + 1] = Math.round(linearToSrgb(clamp(pg, 0.0, 1.0)) * 255).toInt().coerceIn(0, 255).toByte()
            out[i + 2] = Math.round(linearToSrgb(clamp(pb, 0.0, 1.0)) * 255).toInt().coerceIn(0, 255).toByte()
            out[i + 3] = 255.toByte()
        }
        return out
    }

    private fun lum(r: Double, g: Double, b: Double): Double = 0.2126 * r + 0.7152 * g + 0.0722 * b

    private fun clamp(v: Double, min: Double, max: Double): Double = Math.min(max, Math.max(min, v))

    private fun fmt(v: Double): String {
        if (!v.isFinite()) return "0"
        return (Math.round(v * 1e8) / 1e8).toString()
    }

    /** 增益图元数据（hdrgm XMP 的原始值，min/max 为 content boost，非 log2） */
    data class GainMapMetadata(
        val minContentBoost: Double,
        val maxContentBoost: Double,
        val gamma: Double,
        val offsetSdr: Double,
        val offsetHdr: Double,
        val hdrCapacityMin: Double,
        val hdrCapacityMax: Double
    )

    // ============================================================
    //  字节工具
    // ============================================================

    private fun writeU16(buf: ByteArray, off: Int, v: Int) {
        buf[off] = (v shr 8 and 0xFF).toByte()
        buf[off + 1] = (v and 0xFF).toByte()
    }

    private fun writeU32(buf: ByteArray, off: Int, v: Long) {
        buf[off] = (v shr 24 and 0xFF).toByte()
        buf[off + 1] = (v shr 16 and 0xFF).toByte()
        buf[off + 2] = (v shr 8 and 0xFF).toByte()
        buf[off + 3] = (v and 0xFF).toByte()
    }

    private fun writeU32(buf: ByteArray, off: Int, v: Int) = writeU32(buf, off, v.toLong() and 0xFFFFFFFFL)

    private fun readU16(buf: ByteArray, off: Int): Int =
        ((buf[off].toInt() and 0xFF) shl 8) or (buf[off + 1].toInt() and 0xFF)

    private fun readU32(buf: ByteArray, off: Int): Long =
        ((buf[off].toLong() and 0xFF) shl 24) or
                ((buf[off + 1].toLong() and 0xFF) shl 16) or
                ((buf[off + 2].toLong() and 0xFF) shl 8) or
                (buf[off + 3].toLong() and 0xFF)

    private fun concat(vararg parts: ByteArray): ByteArray {
        val out = ByteArrayOutputStream()
        for (p in parts) out.write(p)
        return out.toByteArray()
    }

    private fun s15(v: Double): Int = Math.round(v * 65536).toInt()

    // ============================================================
    //  sRGB ICC 配置文件（程序化生成）
    // ============================================================

    fun buildSrgbIcc(): ByteArray {
        // 参数曲线类型 4：X>=d 时 Y=(aX+b)^g，否则 Y=cX（sRGB 分段曲线）
        fun paraType4(g: Double, a: Double, b: Double, c: Double, d: Double): ByteArray {
            val buf = ByteArray(32)
            "para".toByteArray(Charsets.US_ASCII).copyInto(buf, 0)
            writeU16(buf, 8, 4) // function type = 4
            writeU32(buf, 12, s15(g).toLong())
            writeU32(buf, 16, s15(a).toLong())
            writeU32(buf, 20, s15(b).toLong())
            writeU32(buf, 24, s15(c).toLong())
            writeU32(buf, 28, s15(d).toLong())
            return buf
        }

        fun xyzType(x: Double, y: Double, z: Double): ByteArray {
            val buf = ByteArray(20)
            "XYZ ".toByteArray(Charsets.US_ASCII).copyInto(buf, 0)
            writeU32(buf, 8, s15(x).toLong())
            writeU32(buf, 12, s15(y).toLong())
            writeU32(buf, 16, s15(z).toLong())
            return buf
        }

        fun textType(sig: String, str: String): ByteArray {
            val s = str.toByteArray(Charsets.UTF_8)
            val buf = ByteArray(12 + s.size)
            sig.toByteArray(Charsets.US_ASCII).copyInto(buf, 0)
            writeU32(buf, 8, s.size.toLong())
            s.copyInto(buf, 12)
            return buf
        }

        val srg = paraType4(2.4, 1.0 / 1.055, 0.055 / 1.055, 1.0 / 12.92, 0.04045)
        // 按签名排序（ICC v4 要求）
        val tags = arrayOf(
            arrayOf("bTRC", srg), arrayOf("bXYZ", xyzType(0.143051, 0.060608, 0.713913)),
            arrayOf("cprt", textType("text", "Public Domain")), arrayOf("desc", textType("desc", "sRGB")),
            arrayOf("gTRC", srg), arrayOf("gXYZ", xyzType(0.385113, 0.716879, 0.097109)),
            arrayOf("rTRC", srg), arrayOf("rXYZ", xyzType(0.436041, 0.222485, 0.01392)),
            arrayOf("wtpt", xyzType(0.9642, 1.0, 0.8249)),
        ).sortedBy { it[0] as String }

        val header = ByteArray(128)
        "lcms".toByteArray(Charsets.US_ASCII).copyInto(header, 4)
        writeU32(header, 8, 0x04300000L) // ICC v4.3
        "mntr".toByteArray(Charsets.US_ASCII).copyInto(header, 12)
        "RGB ".toByteArray(Charsets.US_ASCII).copyInto(header, 16)
        "XYZ ".toByteArray(Charsets.US_ASCII).copyInto(header, 20)
        "acsp".toByteArray(Charsets.US_ASCII).copyInto(header, 36)
        "MSFT".toByteArray(Charsets.US_ASCII).copyInto(header, 40)
        writeU32(header, 64, 1L) // rendering intent = 1
        writeU32(header, 68, s15(0.9642).toLong()) // PCS illuminant X
        writeU32(header, 72, s15(1.0).toLong())
        writeU32(header, 76, s15(0.8249).toLong())
        "lcms".toByteArray(Charsets.US_ASCII).copyInto(header, 80)

        val tagTableStart = 128
        val tagTableSize = 4 + tags.size * 12
        val tagTable = ByteArray(tagTableSize)
        writeU32(tagTable, 0, tags.size.toLong())

        val chunks = ByteArrayOutputStream()
        var offset = tagTableStart + tagTableSize
        tags.forEachIndexed { i, tag ->
            val data = tag[1] as ByteArray
            writeU32(tagTable, 4 + i * 12, offset.toLong())
            writeU32(tagTable, 8 + i * 12, data.size.toLong())
            chunks.write(data)
            offset += data.size
        }

        val profile = concat(header, tagTable, chunks.toByteArray())
        writeU32(profile, 0, profile.size.toLong())
        return profile
    }

    // ============================================================
    //  JPEG 段构建
    // ============================================================

    private fun buildApp1Xmp(xmp: String): ByteArray {
        val ns = "http://ns.adobe.com/xap/1.0/\u0000".toByteArray(Charsets.UTF_8)
        val payload = concat(ns, xmp.toByteArray(Charsets.UTF_8))
        val seg = ByteArray(4 + payload.size)
        writeU16(seg, 0, 0xFFE1)
        writeU16(seg, 2, 2 + payload.size)
        payload.copyInto(seg, 4)
        return seg
    }

    private fun buildApp2Icc(icc: ByteArray): ByteArray {
        val sig = "ICC_PROFILE\u0000".toByteArray(Charsets.US_ASCII)
        val payload = concat(sig, byteArrayOf(1, 1), icc)
        val seg = ByteArray(4 + payload.size)
        writeU16(seg, 0, 0xFFE2)
        writeU16(seg, 2, 2 + payload.size)
        payload.copyInto(seg, 4)
        return seg
    }

    /** 构建 JFIF APP0 段（与真实 Ultra HDR 文件一致） */
    private fun buildJfifApp0(): ByteArray = byteArrayOf(
        0xFF.toByte(), 0xE0.toByte(), 0x00, 0x10,
        0x4A, 0x46, 0x49, 0x46, 0x00, 0x01, 0x01, 0x00,
        0x00, 0x01, 0x00, 0x01, 0x00, 0x00
    )

    /** MPF APP2 载荷（CIPA DC-x007 标准结构，含 TIFF 幻数 0x002A，全部大端） */
    private fun buildMpfPayload(
        primarySize: Long,
        secondarySize: Long,
        secondaryOffset: Long
    ): ByteArray {
        val buf = ByteArray(86)
        "MPF\u0000".toByteArray(Charsets.US_ASCII).copyInto(buf, 0)
        "MM".toByteArray(Charsets.US_ASCII).copyInto(buf, 4)
        writeU16(buf, 6, 0x002A) // TIFF 幻数
        writeU32(buf, 8, 8L) // index IFD offset（相对字节4 -> 绝对12）
        writeU16(buf, 12, 3) // tag count

        // tag1: MPFVersion (0xB000)
        writeU16(buf, 14, 0xB000); writeU16(buf, 16, 0x0007)
        writeU32(buf, 18, 4L)
        "0100".toByteArray(Charsets.US_ASCII).copyInto(buf, 22)

        // tag2: NumberOfImages
        writeU16(buf, 26, 0xB001); writeU16(buf, 28, 0x0004)
        writeU32(buf, 30, 1L); writeU32(buf, 34, 2L)

        // tag3: MPEntry
        writeU16(buf, 38, 0xB002); writeU16(buf, 40, 0x0007)
        writeU32(buf, 42, 32L); writeU32(buf, 46, 50L) // 数据偏移相对字节4 -> 绝对54

        writeU32(buf, 50, 0L) // attribute IFD offset

        // MP Entry 0: 主图像
        writeU32(buf, 54, 0x00030000L) // attr: format=JPEG | type=Primary
        writeU32(buf, 58, primarySize)
        writeU32(buf, 62, 0L)
        writeU16(buf, 66, 0); writeU16(buf, 68, 0)

        // MP Entry 1: 增益图
        writeU32(buf, 70, 0L)
        writeU32(buf, 74, secondarySize)
        writeU32(buf, 78, secondaryOffset)
        writeU16(buf, 82, 0); writeU16(buf, 84, 0)

        return buf
    }

    private fun buildApp2Mpf(payload: ByteArray): ByteArray {
        val seg = ByteArray(4 + payload.size)
        writeU16(seg, 0, 0xFFE2)
        writeU16(seg, 2, 2 + payload.size)
        payload.copyInto(seg, 4)
        return seg
    }

    // ============================================================
    //  XMP 元数据
    // ============================================================

    private fun buildXmpPrimary(gainMapLength: Long): String = """
        <x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="XMP Core 5.5.0">
         <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
          <rdf:Description
           xmlns:Container="http://ns.google.com/photos/1.0/container/"
           xmlns:Item="http://ns.google.com/photos/1.0/container/item/"
           xmlns:hdrgm="http://ns.adobe.com/hdr-gain-map/1.0/"
           hdrgm:Version="1.0">
           <Container:Directory>
            <rdf:Seq>
             <rdf:li rdf:parseType="Resource">
              <Container:Item Item:Semantic="Primary" Item:Mime="image/jpeg"/>
             </rdf:li>
             <rdf:li rdf:parseType="Resource">
              <Container:Item Item:Semantic="GainMap" Item:Mime="image/jpeg" Item:Length="$gainMapLength"/>
             </rdf:li>
            </rdf:Seq>
           </Container:Directory>
          </rdf:Description>
         </rdf:RDF>
        </x:xmpmeta>
    """.trimIndent()

    private fun buildXmpSecondary(meta: GainMapMetadata): String {
        val gmMin = Math.log(meta.minContentBoost) / Math.log(2.0)
        val gmMax = Math.log(meta.maxContentBoost) / Math.log(2.0)
        val hcMin = Math.log(meta.hdrCapacityMin) / Math.log(2.0)
        val hcMax = Math.log(meta.hdrCapacityMax) / Math.log(2.0)
        return """
            <x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="XMP Core 5.5.0">
             <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
              <rdf:Description rdf:about=""
               xmlns:hdrgm="http://ns.adobe.com/hdr-gain-map/1.0/"
               hdrgm:Version="1.0"
               hdrgm:GainMapMin="${fmt(gmMin)}"
               hdrgm:GainMapMax="${fmt(gmMax)}"
               hdrgm:Gamma="${fmt(meta.gamma)}"
               hdrgm:OffsetSDR="${fmt(meta.offsetSdr)}"
               hdrgm:OffsetHDR="${fmt(meta.offsetHdr)}"
               hdrgm:HDRCapacityMin="${fmt(hcMin)}"
               hdrgm:HDRCapacityMax="${fmt(hcMax)}"
               hdrgm:BaseRenditionIsHDR="False">
              </rdf:Description>
             </rdf:RDF>
            </x:xmpmeta>
        """.trimIndent()
    }

    // ============================================================
    //  增益图生成（与 JS 端 computeGainMap 一致）
    // ============================================================

    /**
     * 生成增益图（真正的高光扩展）
     * 主图像 = 原始输入（SDR 底图）；HDR 目标 = SDR 线性 * gainPerPix，
     * 其中 gainPerPix = 1 + (maxBoost-1) * clamp((Y-0.5)*2,0,1)^gamma，高光可超 SDR 白点；
     * highlightStart=0.5：50% 线性亮度以下 gain 恒为 1（严格保中间调/暗部），只扩展高光
     *
     * @param primaryRgba 主图像（SDR 底图，位于主图 ICC 色彩空间）RGBA
     * @param width       图片宽度
     * @param height      图片高度
     * @param settings    转换参数（hdrIntensity=高光扩展档数 EV；gamma=高光掩膜曲线）
     * @return Pair(增益图 8-bit 全长分辨率, GainMapMetadata)
     */
    fun computeGainMap(
        primaryRgba: ByteArray,
        width: Int,
        height: Int,
        settings: ConversionSettings
    ): Pair<ByteArray, GainMapMetadata> {
        val hdrIntensity = settings.hdrIntensity
        val gamma = settings.gamma
        // hdrIntensity 作为高光扩展档数（EV）：maxBoost = 2^hdrIntensity，即 ×SDR 白点
        val userMaxBoost = clamp(Math.pow(2.0, hdrIntensity), 1.0, 64.0)
        // 用户峰值亮度上限：maxBoost 不超过 peakNits/whiteNits（默认 203/1000 → ×4.93）
        val whiteNits = settings.whiteNits ?: 203.0
        val peakNits = settings.peakNits ?: 1000.0
        val peakCap = (peakNits / whiteNits).coerceAtLeast(1.0)
        val maxBoost = Math.min(userMaxBoost, peakCap)
        // 折算回 EV 传给 GPU/CPU，保证两路径用同一个 maxBoost（一致性）
        val effHdrIntensity = Math.log(maxBoost) / Math.log(2.0)
        val highlightStart = 0.5 // 高光掩膜起点（线性亮度）；0.5 以下 gain=1 不变
        val offset = 1.0 / 64.0

        val n = width * height

        // CUDA 加速：GPU 不可用/失败时自动回退 CPU 多线程
        if (HdrGpuJni.isAvailable) {
            try {
                val gm8 = ByteArray(n)
                val minMax = DoubleArray(2)
                if (HdrGpuJni.nativeComputeGainMap(
                        primaryRgba, width, height, effHdrIntensity, gamma, gm8, minMax
                    )
                ) {
                    val minBoost = minMax[0]
                    val maxBoostActual = minMax[1]
                    val meta = GainMapMetadata(
                        minContentBoost = minBoost,
                        maxContentBoost = maxBoostActual,
                        gamma = 1.0,
                        offsetSdr = offset,
                        offsetHdr = offset,
                        hdrCapacityMin = 1.0,
                        hdrCapacityMax = maxBoostActual
                    )
                    return gm8 to meta
                }
            } catch (e: Throwable) {
                System.err.println("[HdrGpuJni] computeGainMap GPU 失败，回退 CPU: ${e.message}")
            }
        }

        val gain = DoubleArray(n)

        // 多线程并行计算增益图（大幅提速，每个线程独立写自己的分片，无竞争）
        val threadCount = Runtime.getRuntime().availableProcessors().coerceIn(1, 16)
        val chunk = (n + threadCount - 1) / threadCount
        val gminArr = DoubleArray(threadCount) { Double.POSITIVE_INFINITY }
        val gmaxArr = DoubleArray(threadCount) { Double.NEGATIVE_INFINITY }
        val workers = (0 until threadCount).map { t ->
            Thread {
                var lmin = Double.POSITIVE_INFINITY
                var lmax = Double.NEGATIVE_INFINITY
                val start = t * chunk
                val end = Math.min(start + chunk, n)
                for (i in start until end) {
                    val base = i * 4
                    val r = srgbToLinear((primaryRgba[base].toInt() and 0xFF) / 255.0)
                    val g = srgbToLinear((primaryRgba[base + 1].toInt() and 0xFF) / 255.0)
                    val b = srgbToLinear((primaryRgba[base + 2].toInt() and 0xFF) / 255.0)
                    val y = lum(r, g, b)
                    // 高光掩膜：亮度 > 50% 的像素从 gain=1 渐变到 maxBoost（50% 以下 gain=1，保中间调）
                    val mask = Math.pow(clamp((y - highlightStart) / (1.0 - highlightStart), 0.0, 1.0), gamma)
                    val gainPerPix = 1.0 + (maxBoost - 1.0) * mask
                    val yhdr = y * gainPerPix
                    val pg = (yhdr + offset) / (y + offset)
                    gain[i] = pg
                    if (pg < lmin) lmin = pg
                    if (pg > lmax) lmax = pg
                }
                gminArr[t] = lmin
                gmaxArr[t] = lmax
            }
        }
        workers.forEach { it.start() }
        workers.forEach { it.join() }

        val gmin = gminArr.min()
        val gmax = gmaxArr.max()
        val minBoost = clamp(Math.min(1.0, gmin), 0.25, 1.0)
        val maxBoostActual = Math.max(1.0, gmax)
        val mapMin = Math.log(minBoost) / Math.log(2.0)
        val mapMax = Math.log(maxBoostActual) / Math.log(2.0)
        val range = Math.max(mapMax - mapMin, 1e-6)

        val gm8 = ByteArray(n)
        // 并行量化
        val quantWorkers = (0 until threadCount).map { t ->
            Thread {
                val start = t * chunk
                val end = Math.min(start + chunk, n)
                for (i in start until end) {
                    val logRec = (Math.log(gain[i]) / Math.log(2.0) - mapMin) / range
                    val rec = clamp(logRec, 0.0, 1.0)
                    gm8[i] = Math.round(rec * 255.0).toInt().coerceIn(0, 255).toByte()
                }
            }
        }
        quantWorkers.forEach { it.start() }
        quantWorkers.forEach { it.join() }

        val meta = GainMapMetadata(
            minContentBoost = minBoost,
            maxContentBoost = maxBoostActual,
            gamma = 1.0,
            offsetSdr = offset,
            offsetHdr = offset,
            hdrCapacityMin = 1.0,
            hdrCapacityMax = maxBoostActual
        )
        return gm8 to meta
    }

    /**
     * 视频链路 2（逐帧增益图）核心：把一张 SDR 帧重建为线性 HDR 的 16-bit PAM。
     *
     * 与图片 Ultra HDR 相同的"高光扩展"模型（highlightStart=0.5，50% 线性亮度以下 gain=1，
     * 严格保中间调/暗部色调），但**直接输出线性 HDR**，跳过 Ultra HDR JPEG 中间格式：
     *   HDR = SDR线性 * gainPerPix，gainPerPix = 1 + (maxBoost-1) * clamp((Y-0.5)*2,0,1)^gamma
     *
     * 输出 P7 PAM（大端 16-bit RGB），ffmpeg 读作 rgb48le 后经 zscale 编码为 HDR10。
     *
     * @param rgba     SDR 帧 RGBA（sRGB 值）
     * @param width    宽度
     * @param height   高度
     * @param settings 转换参数（hdrIntensity=EV，gamma=高光掩膜曲线）
     * @param peak     输出归一化峰值（文件线性 1.0 = peak；编码 npl 应为 100*peak 尼特）
     * @return P7 PAM 字节（大端 16-bit RGB）
     */
    fun reconstructLinearHdrFrame(
        rgba: ByteArray,
        width: Int,
        height: Int,
        settings: ConversionSettings,
        peak: Double = 8.0
    ): ByteArray {
        val hdrIntensity = settings.hdrIntensity
        val gamma = settings.gamma
        val maxBoost = clamp(Math.pow(2.0, hdrIntensity), 1.0, 64.0)
        val highlightStart = 0.5
        // Ultra HDR 式：不应用微调明暗 / RGB（与图片 Ultra HDR 一致，底图=原图），只做增益图高光扩展
        val n = width * height

        val header = "P7\nWIDTH $width\nHEIGHT $height\nDEPTH 3\nMAXVAL 65535\nTUPLTYPE RGB\nENDHDR\n"
            .toByteArray(Charsets.US_ASCII)
        val u16 = ByteArray(n * 3 * 2)

        // 并行度由主进程帧级并发提供（/video-frame 并发 ≈ 核心数），帧内单线程即可：
        // 8 并发 × 1 线程 = 8 线程恰好吃满 8 核，避免 并发×帧内线程 超订与每帧建线程开销。
        val threadCount = 1
        val chunk = (n + threadCount - 1) / threadCount
        val workers = (0 until threadCount).map { t ->
            Thread {
                val start = t * chunk
                val end = Math.min(start + chunk, n)
                for (i in start until end) {
                    val base = i * 4
                    val r = srgbToLinear((rgba[base].toInt() and 0xFF) / 255.0)
                    val g = srgbToLinear((rgba[base + 1].toInt() and 0xFF) / 255.0)
                    val b = srgbToLinear((rgba[base + 2].toInt() and 0xFF) / 255.0)
                    val y = lum(r, g, b)
                    // 高光掩膜：50% 以下 gain=1（保色调），高光渐变到 maxBoost
                    val mask = Math.pow(clamp((y - highlightStart) / (1.0 - highlightStart), 0.0, 1.0), gamma)
                    val gain = 1.0 + (maxBoost - 1.0) * mask
                    val hr = r * gain
                    val hg = g * gain
                    val hb = b * gain
                    val o = i * 6
                    val vr = Math.round(clamp(hr, 0.0, peak) / peak * 65535.0).toInt()
                    val vg = Math.round(clamp(hg, 0.0, peak) / peak * 65535.0).toInt()
                    val vb = Math.round(clamp(hb, 0.0, peak) / peak * 65535.0).toInt()
                    u16[o] = ((vr shr 8) and 0xFF).toByte()
                    u16[o + 1] = (vr and 0xFF).toByte()
                    u16[o + 2] = ((vg shr 8) and 0xFF).toByte()
                    u16[o + 3] = (vg and 0xFF).toByte()
                    u16[o + 4] = ((vb shr 8) and 0xFF).toByte()
                    u16[o + 5] = (vb and 0xFF).toByte()
                }
            }
        }
        workers.forEach { it.start() }
        workers.forEach { it.join() }

        return ByteArray(header.size + u16.size).also { out ->
            System.arraycopy(header, 0, out, 0, header.size)
            System.arraycopy(u16, 0, out, header.size, u16.size)
        }
    }

    /**
     * 视频直接转（图片 jpg_icc 式单层）逐帧重建：sRGB→线性→RGB通道→曝光→伽马→线性 HDR PAM。
     * 与图片 applyHdrTransform 一致（曝光、gamma、rgbAdjustment），但**不做自动伽马**
     * （逐帧自适应会导致视频闪烁），且**不再乘微调明暗**（该滑块已移除）。
     *
     * 曝光 = peak(=峰值/白点 = 2^EV)，不再乘 fineTuneBrightness（微调明暗滑块已移除，
     * 乘 <1 会把 HDR 压暗）。SDR 白（线性 1.0）经曝光后 ≈ 峰值 → 输出才真正 HDR。
     * 旧实现用 hdrIntensity(EV)×fineTune=0.525（默认）把视频压暗成非 HDR（已修复）。
     */
    fun reconstructLinearHdrTransform(
        rgba: ByteArray,
        width: Int,
        height: Int,
        settings: ConversionSettings,
        peak: Double = 8.0
    ): ByteArray {
        // 微调明暗已移除：直接转不再乘 fineTuneBrightness（乘 <1 会把 HDR 压暗）
        val exposure = peak
        val gamma = settings.gamma
        val rAdj = settings.rAdj
        val gAdj = settings.gAdj
        val bAdj = settings.bAdj
        val n = width * height

        val header = "P7\nWIDTH $width\nHEIGHT $height\nDEPTH 3\nMAXVAL 65535\nTUPLTYPE RGB\nENDHDR\n"
            .toByteArray(Charsets.US_ASCII)
        val u16 = ByteArray(n * 3 * 2)

        // 并行度由主进程帧级并发提供（/video-frame 并发 ≈ 核心数），帧内单线程即可：
        // 8 并发 × 1 线程 = 8 线程恰好吃满 8 核，避免 并发×帧内线程 超订与每帧建线程开销。
        val threadCount = 1
        val chunk = (n + threadCount - 1) / threadCount
        val workers = (0 until threadCount).map { t ->
            Thread {
                val start = t * chunk
                val end = Math.min(start + chunk, n)
                for (i in start until end) {
                    val base = i * 4
                    var r = srgbToLinear((rgba[base].toInt() and 0xFF) / 255.0) * rAdj * exposure
                    var g = srgbToLinear((rgba[base + 1].toInt() and 0xFF) / 255.0) * gAdj * exposure
                    var b = srgbToLinear((rgba[base + 2].toInt() and 0xFF) / 255.0) * bAdj * exposure
                    r = Math.pow(Math.max(r, 0.0), gamma)
                    g = Math.pow(Math.max(g, 0.0), gamma)
                    b = Math.pow(Math.max(b, 0.0), gamma)
                    val o = i * 6
                    val vr = Math.round(clamp(r, 0.0, peak) / peak * 65535.0).toInt()
                    val vg = Math.round(clamp(g, 0.0, peak) / peak * 65535.0).toInt()
                    val vb = Math.round(clamp(b, 0.0, peak) / peak * 65535.0).toInt()
                    u16[o] = ((vr shr 8) and 0xFF).toByte()
                    u16[o + 1] = (vr and 0xFF).toByte()
                    u16[o + 2] = ((vg shr 8) and 0xFF).toByte()
                    u16[o + 3] = (vg and 0xFF).toByte()
                    u16[o + 4] = ((vb shr 8) and 0xFF).toByte()
                    u16[o + 5] = (vb and 0xFF).toByte()
                }
            }
        }
        workers.forEach { it.start() }
        workers.forEach { it.join() }

        return ByteArray(header.size + u16.size).also { out ->
            System.arraycopy(header, 0, out, 0, header.size)
            System.arraycopy(u16, 0, out, header.size, u16.size)
        }
    }

    /**
     * 视频直接转首帧预览：与视频输出**完全一致**的色彩管线。
     * sRGB→线性→RGB通道→曝光(峰值)→伽马→线性(Rec.709)→Rec.2020 基色→PQ 编码→8-bit RGBA。
     * 与 reconstructLinearHdrTransform 同构（曝光=峰值、无自动伽马），因此预览像素 = 视频输出首帧的
     * Rec.2020/PQ 值 → Chromium 渲染颜色一致。旧 jpg_icc 预览用 sRGB 编码值标 Rec.2020/PQ ICC（色彩错乱）。
     */
    fun videoDirectPreviewRgba(
        rgba: ByteArray,
        width: Int,
        height: Int,
        settings: ConversionSettings,
        peak: Double,
        whiteNits: Double
    ): ByteArray {
        val exposure = peak
        val gamma = settings.gamma
        val rAdj = settings.rAdj
        val gAdj = settings.gAdj
        val bAdj = settings.bAdj
        val n = width * height
        val out = ByteArray(n * 4)
        // PQ 线性归一化：SDR 白(线性 1.0)=whiteNits 尼特，PQ 满量程=10000 尼特
        val scale = whiteNits / 10000.0
        val threadCount = Runtime.getRuntime().availableProcessors().coerceIn(1, 16)
        val chunk = (n + threadCount - 1) / threadCount
        val workers = (0 until threadCount).map { t ->
            Thread {
                val start = t * chunk
                val end = Math.min(start + chunk, n)
                for (i in start until end) {
                    val base = i * 4
                    var r = srgbToLinear((rgba[base].toInt() and 0xFF) / 255.0) * rAdj * exposure
                    var g = srgbToLinear((rgba[base + 1].toInt() and 0xFF) / 255.0) * gAdj * exposure
                    var b = srgbToLinear((rgba[base + 2].toInt() and 0xFF) / 255.0) * bAdj * exposure
                    r = Math.pow(Math.max(r, 0.0), gamma)
                    g = Math.pow(Math.max(g, 0.0), gamma)
                    b = Math.pow(Math.max(b, 0.0), gamma)
                    // Rec.709 线性 → Rec.2020 线性（与视频 zscale pin=bt709 → p=bt2020 一致）
                    val r2020 = 0.6274038959 * r + 0.3292830384 * g + 0.0433130642 * b
                    val g2020 = 0.0690972894 * r + 0.9195403951 * g + 0.0113623156 * b
                    val b2020 = 0.0163914389 * r + 0.0880133078 * g + 0.8955952528 * b
                    out[base] = Math.round(pqEncode(r2020 * scale) * 255).toInt().coerceIn(0, 255).toByte()
                    out[base + 1] = Math.round(pqEncode(g2020 * scale) * 255).toInt().coerceIn(0, 255).toByte()
                    out[base + 2] = Math.round(pqEncode(b2020 * scale) * 255).toInt().coerceIn(0, 255).toByte()
                    out[base + 3] = 255.toByte()
                }
            }
        }
        workers.forEach { it.start() }
        workers.forEach { it.join() }
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
    //  自动估算 HDR 强度
    // ============================================================

    /** 自动估算结果 */
    data class IntensityEstimate(
        val hdrIntensity: Double,  // 建议滑块值（EV = log2(maxBoost)）
        val maxBoost: Double,      // 建议 max_content_boost（线性倍数）
        val yP995: Double,         // 99.5 分位线性亮度（代表真实高光）
        val hlRatio: Double        // 线性亮度 > 0.5 的高光像素占比
    )

    /**
     * 基于图像亮度分布自动估算 HDR 强度（EV）。
     * 思路：99.5 分位线性亮度代表"真实高光"，据此在 EV 域平滑映射；
     * 高光占比做校正（几乎无高光则保守，高光丰富可略激进）。
     * 实测用户偏好 ≈1.5 EV（×2.8，保高光饱和），强高光图估算落在此附近；
     * 上限 3 EV 仅作手动允许上限，结果与滑块范围一致（0.96..3.0 EV）。
     */
    fun estimateHdrIntensity(rgba: ByteArray, width: Int, height: Int): IntensityEstimate {
        val n = width * height
        val hist = IntArray(256)
        var hlCount = 0
        for (i in 0 until n) {
            val base = i * 4
            val r = srgbToLinear((rgba[base].toInt() and 0xFF) / 255.0)
            val g = srgbToLinear((rgba[base + 1].toInt() and 0xFF) / 255.0)
            val b = srgbToLinear((rgba[base + 2].toInt() and 0xFF) / 255.0)
            val y = lum(r, g, b)
            if (y > 0.5) hlCount++
            val bin = Math.min(255, (y * 255.0).toInt())
            hist[bin]++
        }
        // 99.5 分位线性亮度（从高到低累计 0.5% 像素，抗单点高亮噪声）
        val cutoff = Math.max(1, Math.round(n * 0.005).toInt())
        var acc = 0
        var yP995 = 0.0
        for (b in 255 downTo 0) {
            acc += hist[b]
            if (acc >= cutoff) {
                yP995 = (b + 0.5) / 255.0
                break
            }
        }
        val hlRatio = hlCount.toDouble() / Math.max(1, n)

        // 高光强度 0..1
        val yNorm = clamp((yP995 - 0.25) / 0.75, 0.0, 1.0)
        // EV 平滑映射（实测用户偏好 ≈1.5 EV / ×2.8，保高光饱和）：
        // 几乎无高光 -> ~0.8 EV（×1.7），强高光 -> 1.5 EV（×2.8）
        var ev = 0.8 + 0.7 * yNorm
        // 高光占比校正
        if (hlRatio < 0.002) ev *= 0.9
        else if (hlRatio > 0.02) ev *= 1.05
        val hdrIntensity = clamp(ev, 0.96, 3.0)
        val maxBoost = Math.pow(2.0, hdrIntensity)
        return IntensityEstimate(hdrIntensity, maxBoost, yP995, hlRatio)
    }

    /** 双线性下采样（单通道） */
    fun downscaleBilinear(src: ByteArray, sw: Int, sh: Int, dw: Int, dh: Int): ByteArray {
        val out = ByteArray(dw * dh)
        val xs = sw.toDouble() / dw
        val ys = sh.toDouble() / dh
        for (y in 0 until dh) {
            val sy = y * ys
            val y0 = Math.min(Math.floor(sy).toInt(), sh - 1)
            val y1 = Math.min(y0 + 1, sh - 1)
            val fy = sy - y0
            for (x in 0 until dw) {
                val sx = x * xs
                val x0 = Math.min(Math.floor(sx).toInt(), sw - 1)
                val x1 = Math.min(x0 + 1, sw - 1)
                val fx = sx - x0
                val v =
                    (src[y0 * sw + x0].toInt() and 0xFF) * (1 - fx) * (1 - fy) +
                            (src[y0 * sw + x1].toInt() and 0xFF) * fx * (1 - fy) +
                            (src[y1 * sw + x0].toInt() and 0xFF) * (1 - fx) * fy +
                            (src[y1 * sw + x1].toInt() and 0xFF) * fx * fy
                out[y * dw + x] = Math.round(v).toInt().coerceIn(0, 255).toByte()
            }
        }
        return out
    }

    // ============================================================
    //  JPEG 编码 / 重组
    // ============================================================

    private fun encodeJpegRgb(rgba: ByteArray, width: Int, height: Int, quality: Float): ByteArray {
        val img = BufferedImage(width, height, BufferedImage.TYPE_INT_RGB)
        for (i in 0 until width * height) {
            val r = rgba[i * 4].toInt() and 0xFF
            val g = rgba[i * 4 + 1].toInt() and 0xFF
            val b = rgba[i * 4 + 2].toInt() and 0xFF
            img.setRGB(i % width, i / width, (0xFF shl 24) or (r shl 16) or (g shl 8) or b)
        }
        return encodeJpeg(img, quality)
    }

    private fun encodeJpegGray(gray: ByteArray, width: Int, height: Int, quality: Float): ByteArray {
        val img = BufferedImage(width, height, BufferedImage.TYPE_BYTE_GRAY)
        val raster = img.raster
        for (i in 0 until width * height) {
            raster.setSample(i % width, i / width, 0, gray[i].toInt() and 0xFF)
        }
        return encodeJpeg(img, quality)
    }

    private fun encodeJpeg(img: BufferedImage, quality: Float): ByteArray {
        val baos = ByteArrayOutputStream()
        val writer = ImageIO.getImageWritersByFormatName("jpg").next()
        val param = writer.defaultWriteParam
        param.compressionMode = ImageWriteParam.MODE_EXPLICIT
        param.compressionQuality = quality
        writer.output = MemoryCacheImageOutputStream(baos)
        writer.write(null, IIOImage(img, null, null), param)
        writer.dispose()
        return baos.toByteArray()
    }

    /** 去除 JPEG 开头的 APPn/COM 段，返回 body（DQT/SOF/DHT/SOS...） */
    private fun stripJpegAppSegments(jpeg: ByteArray): ByteArray {
        var off = 2
        while (off + 4 <= jpeg.size) {
            if ((jpeg[off].toInt() and 0xFF) != 0xFF) break
            val marker = jpeg[off + 1].toInt() and 0xFF
            if (marker in 0xE0..0xEF || marker == 0xFE) {
                off += 2 + readU16(jpeg, off + 2)
            } else break
        }
        return jpeg.copyOfRange(off, jpeg.size)
    }

    /**
     * 重组主图像：
     *   SOI + APP0 + APP1(XMP) + APP2(ICC) + DQT/SOF/DHT + APP2(MPF) + SOS+data+EOI
     * @return Pair<bufferWithoutMpf, posBeforeMpf>
     */
    private fun reorderPrimary(
        primaryJpeg: ByteArray,
        app1Xmp: ByteArray,
        app2Icc: ByteArray
    ): Pair<ByteArray, Int> {
        val headApp = ArrayList<ByteArray>()
        var off = 2
        while (off + 4 <= primaryJpeg.size) {
            if ((primaryJpeg[off].toInt() and 0xFF) != 0xFF) break
            val marker = primaryJpeg[off + 1].toInt() and 0xFF
            if (marker in 0xE0..0xEF) {
                val len = readU16(primaryJpeg, off + 2)
                headApp.add(primaryJpeg.copyOfRange(off, off + 2 + len))
                off += 2 + len
            } else break
        }

        val bodyStart = off
        var p = off
        var sosOffset = -1
        while (p + 4 <= primaryJpeg.size) {
            if ((primaryJpeg[p].toInt() and 0xFF) != 0xFF) {
                p++; continue
            }
            val marker = primaryJpeg[p + 1].toInt() and 0xFF
            if (marker == 0xFF) {
                p++; continue
            }
            if (marker in 0xD0..0xD7) {
                p += 2; continue
            }
            if (marker == 0xDA) {
                sosOffset = p; break
            }
            if (marker == 0xD9) break
            p += 2 + readU16(primaryJpeg, p + 2)
        }

        val beforeSos = if (sosOffset >= 0) primaryJpeg.copyOfRange(bodyStart, sosOffset)
        else primaryJpeg.copyOfRange(bodyStart, primaryJpeg.size)
        val sosAndRest = if (sosOffset >= 0) primaryJpeg.copyOfRange(sosOffset, primaryJpeg.size) else ByteArray(0)

        // 确保开头恰好一个 JFIF APP0（与真实 Ultra HDR 文件一致）
        val nonJfif = headApp.filter {
            !(it.size >= 6 && (it[4].toInt() and 0xFF) == 0x4A && (it[5].toInt() and 0xFF) == 0x46)
        }
        val head: List<ByteArray> = listOf(buildJfifApp0()) + nonJfif
        val headLenFinal = head.sumOf { it.size }
        val posBeforeMpf = 2 + headLenFinal + app1Xmp.size + app2Icc.size + beforeSos.size

        val out = ByteArrayOutputStream()
        out.write(primaryJpeg.copyOfRange(0, 2)) // SOI
        for (h in head) out.write(h)
        out.write(app1Xmp)
        out.write(app2Icc)
        out.write(beforeSos)
        out.write(sosAndRest)
        return out.toByteArray() to posBeforeMpf
    }

    // ============================================================
    //  总入口
    // ============================================================

    /**
     * 将图像编码为 Ultra HDR JPEG
     *
     * 编码为 Ultra HDR JPEG。
     *
     * 依据规范："SDR 图像的色彩配置定义了 HDR 图像的色彩空间"——主图色彩空间决定 HDR/增益色彩空间。
     * 主图策略（先检测输入色彩空间）：
     *   - settings.primarySrgb=true → 主图保持原始 sRGB 像素 + sRGB ICC（任何查看器看到原图、保色调）
     *   - 检测到输入为 Display-P3 → 主图已是 P3 像素，无需转换，直接标 P3 ICC
     *   - 其他（sRGB / 未声明）→ sRGB 像素转 Display-P3 + P3 ICC（与 Google 文件一致，需查看器色彩管理）
     * 增益图始终基于主图像素（mainRgba）计算，即跟随主图色彩空间。
     *
     * @param primaryRgba 主图像（SDR 渲染 = 原始输入）RGBA
     * @param detectedCs  检测到的输入色彩空间（ColorSpaceDetector.detect）
     * @param onProgress  进度回调 (0..1, 消息)
     * @return 完整 Ultra HDR JPEG 文件字节
     */
    fun encode(
        primaryRgba: ByteArray,
        width: Int,
        height: Int,
        settings: ConversionSettings,
        detectedCs: InputColorSpace? = null,
        onProgress: ((Double, String) -> Unit)? = null
    ): ByteArray {
        val gainMapScale = 4
        onProgress?.invoke(
            0.15,
            when {
                settings.primarySrgb -> "主图像素保持 sRGB"
                detectedCs == InputColorSpace.DISPLAY_P3 -> "输入已是 Display-P3，主图保持"
                else -> "转换像素到 Display-P3"
            }
        )
        // 主图像素：默认 sRGB→Display-P3（与 P3 ICC 标签一致，避免 sRGB 像素被当 P3 渲染泛白）；
        // 实验 primarySrgb=true → 保持原始 sRGB（主图 ICC 也换 sRGB → 任何查看器看到原图、保色调）；
        // 主图像素（mainRgba）：按主图 ICC 对应的色彩空间就位
        //  - primarySrgb=true → 保持 sRGB（主图 ICC 换 sRGB，任何查看器看到原图）
        //  - 输入已是 Display-P3 → 无需转换
        //  - 其他 → sRGB 像素转 Display-P3（与 P3 ICC 一致，避免被当 P3 渲染泛白）
        val mainRgba = when {
            settings.primarySrgb -> primaryRgba
            detectedCs == InputColorSpace.DISPLAY_P3 -> primaryRgba
            else -> srgbRgbaToDisplayP3Rgba(primaryRgba, width, height)
        }
        val primaryIcc = if (settings.primarySrgb) UHDR_GAINMAP_ICC else UHDR_PRIMARY_ICC

        // 1. 增益图（SDR 基准 = mainRgba 的线性亮度，即主图色彩空间）
        onProgress?.invoke(0.30, "生成高光扩展增益图（多线程）")
        val (gm8, meta) = computeGainMap(mainRgba, width, height, settings)
        val gmW = Math.max(1, width / gainMapScale)
        val gmH = Math.max(1, height / gainMapScale)
        val down = downscaleBilinear(gm8, width, height, gmW, gmH)

        // 2. 增益图灰度基线 JPEG（ImageIO TYPE_BYTE_GRAY 输出 1 分量 SOF0）
        onProgress?.invoke(0.65, "编码增益图")
        val quality = settings.quality.toFloat().coerceIn(0.1f, 1.0f)
        val gmJpeg = encodeJpegGray(down, gmW, gmH, quality)

        // 3. 次图像: 独立 JPEG = SOI + APP0(JFIF) + APP1(hdrgm XMP) + APP2(ICC) + body
        val secondary = concat(
            byteArrayOf(0xFF.toByte(), 0xD8.toByte()),
            buildJfifApp0(),
            buildApp1Xmp(buildXmpSecondary(meta)),
            buildApp2Icc(UHDR_GAINMAP_ICC),
            stripJpegAppSegments(gmJpeg)
        )
        val secondarySize = secondary.size.toLong()

        // 4. 主图像 JPEG（基线，像素 = mainRgba，与主图 ICC 色彩空间一致）
        onProgress?.invoke(0.85, "编码 SDR 主图像")
        val primaryJpeg = encodeJpegRgb(mainRgba, width, height, quality)

        // 5. 主图像 XMP + ICC
        val app1Xmp = buildApp1Xmp(buildXmpPrimary(secondarySize))
        val app2Icc = buildApp2Icc(primaryIcc)

        // 6. 重组主图像 + 计算 MPF 偏移
        val (bufferWithoutMpf, posBeforeMpf) = reorderPrimary(primaryJpeg, app1Xmp, app2Icc)
        val mpfPayloadLen = 86
        val mpfApp2Len = 4 + mpfPayloadLen
        val sosAndRestLen = bufferWithoutMpf.size - posBeforeMpf
        val primarySize = (posBeforeMpf + mpfApp2Len + sosAndRestLen).toLong()
        val secondaryOffset = primarySize - posBeforeMpf - 8

        val mpfApp2 = buildApp2Mpf(buildMpfPayload(primarySize, secondarySize, secondaryOffset))
        val primary = concat(
            bufferWithoutMpf.copyOfRange(0, posBeforeMpf),
            mpfApp2,
            bufferWithoutMpf.copyOfRange(posBeforeMpf, bufferWithoutMpf.size)
        )

        // 7. 拼接
        return concat(primary, secondary)
    }
}
