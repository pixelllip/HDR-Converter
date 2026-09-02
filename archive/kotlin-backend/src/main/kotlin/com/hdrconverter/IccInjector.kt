package com.hdrconverter

import java.io.ByteArrayOutputStream
import java.io.File
import java.util.zip.Deflater

/**
 * ICC 色彩配置文件注入
 *
 * 支持 PNG (iCCP chunk) 和 JPEG (APP2 marker) 格式
 */
object IccInjector {

    // ============================================================
    //  PNG iCCP 注入
    // ============================================================

    /**
     * 将 ICC Profile 注入到 PNG 文件中
     *
     * PNG 结构:
     *   Signature (8 bytes)
     *   IHDR chunk
     *   iCCP chunk (插入在 IHDR 和 IDAT 之间)
     *   IDAT chunk(s)
     *   IEND chunk
     */
    fun injectIccIntoPng(pngBuffer: ByteArray, iccProfileBuffer: ByteArray): ByteArray {
        val signature = pngBuffer.copyOfRange(0, 8)
        val chunks = mutableListOf<PngChunk>()
        var offset = 8

        while (offset < pngBuffer.size) {
            val length = readUInt32BE(pngBuffer, offset)
            val type = String(pngBuffer, offset + 4, 4, Charsets.US_ASCII)
            val data = pngBuffer.copyOfRange(offset + 8, offset + 8 + length)
            chunks.add(PngChunk(type, data))
            offset += 12 + length
        }

        // 构建 iCCP chunk: "BT.2020\0" + compression_method(0) + deflate(icc)
        val profileName = "BT.2020\u0000".toByteArray(Charsets.UTF_8)
        val compressedProfile = deflate(iccProfileBuffer)
        val iccChunkData = profileName + byteArrayOf(0) + compressedProfile

        // 在第一个 IDAT 之前插入 iCCP
        val idatIndex = chunks.indexOfFirst { it.type == "IDAT" }
        val insertIndex = if (idatIndex >= 0) idatIndex else chunks.size
        chunks.add(insertIndex, PngChunk("iCCP", iccChunkData))

        // 重建 PNG
        val output = ByteArrayOutputStream()
        output.write(signature)
        for (chunk in chunks) {
            output.write(createPngChunk(chunk.type, chunk.data))
        }
        return output.toByteArray()
    }

    private data class PngChunk(val type: String, val data: ByteArray)

    private fun createPngChunk(type: String, data: ByteArray): ByteArray {
        val typeBytes = type.toByteArray(Charsets.US_ASCII)
        val lengthBuf = ByteArray(4)
        writeUInt32BE(lengthBuf, 0, data.size)

        val crcInput = typeBytes + data
        val crc = crc32(crcInput)

        val crcBuf = ByteArray(4)
        writeUInt32BE(crcBuf, 0, crc)

        return lengthBuf + typeBytes + data + crcBuf
    }

    // ============================================================
    //  JPEG APP2 ICC_PROFILE 注入
    // ============================================================

    /**
     * 将 ICC Profile 注入到 JPEG 文件中
     *
     * JPEG APP2 段格式:
     *   FF E2 [length] "ICC_PROFILE\0" [seq_num] [total_num] [icc_data]
     *
     * 位置约定：APP2 插在 SOI 之后、所有前置 APP/COM 段之后（即 DQT/SOF/SOS 之前），
     * 这是标准且各查看器都接受的位置；切勿插到 SOS 之后的熵数据里。
     */
    fun injectIccIntoJpeg(jpegBuffer: ByteArray, iccProfileBuffer: ByteArray): ByteArray {
        val app2Segment = buildIccApp2Segment(iccProfileBuffer)
        // 找到前置 APP/COM 段结束位置
        var insertPos = 2
        while (insertPos + 4 <= jpegBuffer.size) {
            val marker = ((jpegBuffer[insertPos].toInt() and 0xFF) shl 8) or (jpegBuffer[insertPos + 1].toInt() and 0xFF)
            val isApp = marker in 0xFFE0..0xFFEF
            val isCom = marker == 0xFFFE
            if (!isApp && !isCom) break
            val segLen = ((jpegBuffer[insertPos + 2].toInt() and 0xFF) shl 8) or (jpegBuffer[insertPos + 3].toInt() and 0xFF)
            insertPos += 2 + segLen
        }
        val out = ByteArray(jpegBuffer.size + app2Segment.size)
        System.arraycopy(jpegBuffer, 0, out, 0, insertPos)
        System.arraycopy(app2Segment, 0, out, insertPos, app2Segment.size)
        System.arraycopy(jpegBuffer, insertPos, out, insertPos + app2Segment.size, jpegBuffer.size - insertPos)
        return out
    }

    private fun buildIccApp2Segment(iccProfileBuffer: ByteArray): ByteArray {
        val sig = "ICC_PROFILE\u0000".toByteArray(Charsets.UTF_8) // 12 bytes
        // 长度字段 = 2（长度本身）+ sig(12) + seq/total(2) + icc(N)，不含 marker 2 字节
        val segLen = 2 + sig.size + 2 + iccProfileBuffer.size

        val header = ByteArray(2 + 2 + sig.size + 2)
        var off = 0
        writeUInt16BE(header, off, 0xFFE2); off += 2   // APP2 marker
        writeUInt16BE(header, off, segLen); off += 2    // segment length (includes self)
        System.arraycopy(sig, 0, header, off, sig.size); off += sig.size // "ICC_PROFILE\0"
        header[off++] = 1                                 // sequence number
        header[off++] = 1                                 // total number of segments

        return header + iccProfileBuffer
    }

    // ============================================================
    //  读取 ICC Profile
    // ============================================================

    fun readIccProfile(iccProfilePath: String): ByteArray {
        return File(iccProfilePath).readBytes()
    }

    // ============================================================
    //  工具函数
    // ============================================================

    private fun readUInt32BE(buf: ByteArray, offset: Int): Int {
        return ((buf[offset].toInt() and 0xFF) shl 24) or
                ((buf[offset + 1].toInt() and 0xFF) shl 16) or
                ((buf[offset + 2].toInt() and 0xFF) shl 8) or
                (buf[offset + 3].toInt() and 0xFF)
    }

    private fun writeUInt32BE(buf: ByteArray, offset: Int, value: Int) {
        buf[offset] = (value shr 24 and 0xFF).toByte()
        buf[offset + 1] = (value shr 16 and 0xFF).toByte()
        buf[offset + 2] = (value shr 8 and 0xFF).toByte()
        buf[offset + 3] = (value and 0xFF).toByte()
    }

    private fun writeUInt16BE(buf: ByteArray, offset: Int, value: Int) {
        buf[offset] = (value shr 8 and 0xFF).toByte()
        buf[offset + 1] = (value and 0xFF).toByte()
    }

    /**
     * CRC-32 校验 (与 PNG 标准一致)
     */
    private fun crc32(data: ByteArray): Int {
        var crc = -1 // 0xFFFFFFFF as signed int
        val poly = 0xEDB88320.toInt() // polynomial as signed int
        for (byte in data) {
            crc = crc xor (byte.toInt() and 0xFF)
            for (i in 0 until 8) {
                crc = if ((crc and 1) == 1) {
                    (crc ushr 1) xor poly
                } else {
                    crc ushr 1
                }
            }
        }
        return crc xor -1 // 0xFFFFFFFF as signed int
    }

    /**
     * Deflate 压缩 (用于 PNG iCCP chunk)
     */
    private fun deflate(data: ByteArray): ByteArray {
        val deflater = Deflater(Deflater.DEFAULT_COMPRESSION, false) // zlib format (with header + Adler-32)
        val output = ByteArrayOutputStream()
        deflater.setInput(data)
        deflater.finish()
        val buffer = ByteArray(8192)
        while (!deflater.finished()) {
            val count = deflater.deflate(buffer)
            output.write(buffer, 0, count)
        }
        deflater.end()
        return output.toByteArray()
    }
}
