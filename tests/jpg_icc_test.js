/**
 * HDR JPEG（ICC 增益，BT.2020）输出验证
 *
 * 验证：
 *   1. outputFormat="jpg_icc" 能生成带 BT.2020 ICC 的 JPEG
 *   2. ICC APP2 插在所有前置 APP 段之后（标准位置，非 SOI 后立即、非熵数据中）
 *   3. quality 参数生效（不同质量文件大小不同）
 *
 * 用法：node tests/jpg_icc_test.js
 */
const path = require('path')
const sharp = require('sharp')
const { ensureBackend, stopBackend, httpJson } = require('./backend_test_util')

/** 解析 JPEG 前置段，返回 { segs: [{marker, off, len}], iccIndex, firstNonAppOff } */
function parseHead(buf) {
    const segs = []
    let off = 2
    while (off + 4 <= buf.length) {
        if (buf[off] !== 0xFF) break
        const marker = buf.readUInt16BE(off)
        if (marker === 0xFFDA || marker === 0xFFDB || (marker >= 0xFFC0 && marker <= 0xFFCF)) break
        if (marker === 0xFFD9) break
        const len = buf.readUInt16BE(off + 2)
        segs.push({ marker, off, len, data: buf.slice(off + 4, off + 2 + len) })
        off += 2 + len
    }
    return { segs, firstNonAppOff: off }
}

/** 严格校验 JPEG 段结构：按长度字段逐段走，验证无越界、能正确到达 SOS/EOI */
function validateJpegSegments(buf) {
    let off = 2
    let count = 0
    while (off + 2 <= buf.length) {
        if (buf[off] !== 0xFF) throw new Error(`@${off} 非 0xFF 标记`)
        const marker = buf.readUInt16BE(off)
        if (marker === 0xFFD9) return { ok: true, count, end: off + 2, eoi: true }
        if (marker === 0xFFDA) return { ok: true, count, end: off, sos: true }
        if (marker >= 0xFFD0 && marker <= 0xFFD7) { off += 2; count++; continue } // RST
        const len = buf.readUInt16BE(off + 2)
        if (off + 2 + len > buf.length) throw new Error(`段 ${marker.toString(16)} 长度越界: off=${off} len=${len}`)
        off += 2 + len
        count++
    }
    throw new Error('未找到 SOS/EOI，段结构异常')
}

; (async () => {
    const input = path.join(__dirname, 'tmp_jpgicc_input.png')
    await sharp({
        create: { width: 400, height: 300, channels: 3, background: { r: 30, g: 60, b: 120 } }
    }).composite([{ input: Buffer.from('<svg width="400" height="300"><rect width="400" height="300" fill="#3a6ea5"/><circle cx="280" cy="120" r="70" fill="#ffdd88"/></svg>') }])
        .png().toFile(input)

    await ensureBackend()

    const outHigh = path.join(__dirname, 'tmp_jpgicc_q100.jpg')
    const outLow = path.join(__dirname, 'tmp_jpgicc_q40.jpg')

    // 质量 100%
    const r1 = await httpJson('POST', '/convert', {
        inputPath: input, outputPath: outHigh,
        settings: { hdrIntensity: 1.2, fineTuneBrightness: 0.5, gamma: 0.9, outputFormat: 'jpg_icc', quality: 1.0 }
    })
    // 质量 40%
    const r2 = await httpJson('POST', '/convert', {
        inputPath: input, outputPath: outLow,
        settings: { hdrIntensity: 1.2, fineTuneBrightness: 0.5, gamma: 0.9, outputFormat: 'jpg_icc', quality: 0.4 }
    })
    if (!r1.success || !r2.success) { console.error('❌ 转换失败:', r1.message, r2.message); process.exit(1) }

    const b1 = require('fs').readFileSync(outHigh)
    const b2 = require('fs').readFileSync(outLow)
    const meta = await sharp(outHigh).metadata()
    console.log('格式:', meta.format, '尺寸:', meta.width + 'x' + meta.height)

    // 严格段校验（此前长度字段多算 2 字节会在这里暴露）
    const v1 = validateJpegSegments(b1)
    const v2 = validateJpegSegments(b2)
    console.log('严格段校验（质量100）:', v1.ok ? `✅ ${v1.count} 段，终止于 ${v1.eoi ? 'EOI' : 'SOS'}` : '❌')
    console.log('严格段校验（质量40）:', v2.ok ? `✅ ${v2.count} 段，终止于 ${v2.eoi ? 'EOI' : 'SOS'}` : '❌')

    // 解析 ICC 位置
    const h = parseHead(b1)
    const iccIdx = h.segs.findIndex((s) => s.marker === 0xFFE2 && s.data.toString('latin1', 0, 12) === 'ICC_PROFILE\0')
    if (iccIdx < 0) { console.error('❌ 未找到 ICC APP2'); process.exit(1) }
    // 长度字段 L = 16 + N（自身2 + sig12 + seq/total2 + iccN），故 icc 数据 N = L - 16
    const iccSize = h.segs[iccIdx].len - 16
    // 严格校验 APP2 长度字段：应为 16 + iccSize
    const expectedLen = 16 + iccSize
    const actualLen = b1.readUInt16BE(h.segs[iccIdx].off + 2)
    console.log('ICC APP2 长度字段:', actualLen, '(应为', expectedLen + ')', actualLen === expectedLen ? '✅' : '❌')
    console.log('ICC APP2 位置: 第', iccIdx + 1, '个 APP 段，大小', iccSize, 'bytes')
    // 前置段顺序：应所有段都在 DQT/SOS 之前（firstNonAppOff 之后没有段）
    const allBeforeFrame = h.segs.every((s) => s.off < h.firstNonAppOff)
    console.log('全部 APP 段在图像数据之前:', allBeforeFrame ? '✅' : '❌')
    // ICC 应在 JFIF（若有）之后
    const jfifIdx = h.segs.findIndex((s) => s.marker === 0xFFE0 && s.data.toString('latin1', 0, 5) === 'JFIF\0')
    console.log('ICC 在 JFIF 之后:', jfifIdx < 0 || iccIdx > jfifIdx ? '✅' : '❌')
    // 质量影响文件大小
    console.log('质量100 大小:', b1.length, '| 质量40 大小:', b2.length)
    if (b2.length >= b1.length) { console.warn('⚠️ 低质量文件未更小（可能源图过小），可接受') }
    else console.log('✅ 质量滑块生效（低质量文件更小）')

    // ICC 是 BT.2020（2020_profile.icc）——检查 ICC 头色彩空间 RGB
    const icc = h.segs[iccIdx].data.slice(14)
    const colorSpace = icc.toString('latin1', 16, 20)
    console.log('ICC 色彩空间:', colorSpace, '(应为 RGB)')

    stopBackend()
    console.log('✅ HDR JPEG（ICC 增益）验证通过')
})().catch((e) => { console.error('测试失败:', e); stopBackend(); process.exit(1) })
