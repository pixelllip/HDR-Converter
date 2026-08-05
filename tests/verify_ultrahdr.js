/**
 * 验证 Ultra HDR JPEG 输出是否符合规范
 *
 * 检查项:
 *  1. 主图像 XMP: GContainer 目录 + hdrgm:Version="1.0"
 *  2. 主图像 ICC: APP2 ICC_PROFILE，色彩空间应为 RGB（sRGB），不再是 Rec.2020/PQ
 *  3. MPF: APP2 "MPF\0"，2 个 MP Entry（主图 + 增益图）
 *  4. 次图像（增益图）: SOI + APP1(hdrgm XMP: GainMapMin/Max/Gamma/Offset...)
 *  5. 增益图为灰度 JPEG
 */
const fs = require('fs')
const path = require('path')
const sharp = require('sharp')
const { convertImage, stopBackend } = require('./backend_test_util')

function readStr(buf, off, len) {
  let s = ''
  for (let i = 0; i < len && off + i < buf.length; i++) {
    const c = buf[off + i]
    if (c === 0) break
    s += String.fromCharCode(c)
  }
  return s
}

/** 解析 JPEG 段，返回 { app1Xmp, app2Icc, mpf, secondary } */
function parseUltraHdr(buf) {
  const result = { segments: [] }
  let off = 2
  let firstSos = -1
  // 主图像段
  while (off + 4 <= buf.length) {
    if (buf[off] !== 0xFF) break
    const marker = buf.readUInt16BE(off)
    if (marker === 0xFFDA) { firstSos = off; break }
    if (marker === 0xFFD9) break
    const len = buf.readUInt16BE(off + 2)
    const data = buf.slice(off + 4, off + 2 + len)
    result.segments.push({ marker: marker.toString(16), off, len, data })
    if (marker === 0xFFE1 && readStr(data, 0, 29) === 'http://ns.adobe.com/xap/1.0/') {
      result.primaryXmp = data.toString('utf8', 29)
    }
    if (marker === 0xFFE2 && data.toString('latin1', 0, 12) === 'ICC_PROFILE\0') {
      result.icc = data.slice(14)
    }
    if (marker === 0xFFE2 && data.toString('latin1', 0, 4) === 'MPF\0') {
      result.mpf = data
    }
    off += 2 + len
  }
  // 找主图像 EOI（SOS 之后熵数据中找 FF D9）
  let eoi = -1
  if (firstSos >= 0) {
    let p = firstSos + 2
    const len = buf.readUInt16BE(firstSos + 2)
    p += 2 + len
    for (let i = p; i < buf.length - 1; i++) {
      if (buf[i] === 0xFF && buf[i + 1] === 0x00) { i++; continue }
      if (buf[i] === 0xFF && buf[i + 1] === 0xD9) { eoi = i + 2; break }
    }
  }
  // 次图像（增益图）从 EOI 之后开始
  if (eoi >= 0) {
    const sec = buf.slice(eoi)
    result.secondary = sec
    // 解析次图像 APP1 XMP
    let so = 2
    while (so + 4 <= sec.length) {
      if (sec[so] !== 0xFF) break
      const marker = sec.readUInt16BE(so)
      if (marker === 0xFFDA) break
      const len = sec.readUInt16BE(so + 2)
      if (marker === 0xFFE1 && readStr(sec, so + 4, 29) === 'http://ns.adobe.com/xap/1.0/') {
        result.gainMapXmp = sec.toString('utf8', so + 4 + 29, so + 2 + len)
      }
      so += 2 + len
    }
  }
  return result
}

function parseMpf(mpf) {
  const entries = []
  // MP Entry 数组起点 = 绝对 52（字节0为 MPF\0 签名，字节4为 TIFF 头起点，条目在绝对52）
  const entriesOff = 52
  for (let i = 0; i < 2; i++) {
    const e = entriesOff + i * 16
    entries.push({
      attr: mpf.readUInt32BE(e),
      size: mpf.readUInt32BE(e + 4),
      offset: mpf.readUInt32BE(e + 8)
    })
  }
  return entries
}

function parseIccInfo(icc) {
  return {
    size: icc.readUInt32BE(0),
    class: readStr(icc, 12, 4),
    colorSpace: readStr(icc, 16, 4),
    pcs: readStr(icc, 20, 4),
    version: icc.readUInt32BE(8)
  }
}

; (async () => {
  // 1. 生成测试输入（渐变 + 高光区）
  const inputPath = path.join(__dirname, 'tmp_uhdr_input.png')
  await sharp({
    create: { width: 640, height: 360, channels: 3, background: { r: 20, g: 60, b: 120 } }
  })
    .composite([{
      input: Buffer.from(
        `<svg width="640" height="360"><defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#101010"/><stop offset="1" stop-color="#ffffff"/></linearGradient><radialGradient id="r"><stop offset="0" stop-color="#ffffff"/><stop offset="0.3" stop-color="#f0f0f0"/><stop offset="1" stop-color="#00000000"/></radialGradient></defs><rect width="640" height="360" fill="url(#g)"/><circle cx="320" cy="180" r="90" fill="url(#r)"/></svg>`
      )
    }])
    .png()
    .toFile(inputPath)

  // 2. 转换
  const outPath = path.join(__dirname, 'tmp_uhdr_output.jpg')
  const res = await convertImage({
    inputPath,
    outputPath: outPath,
    settings: { hdrIntensity: 1.18, fineTuneBrightness: 0.3, gamma: 0.9, outputFormat: 'jpg' }
  })
  console.log('convert result:', res.success ? 'OK' : 'FAIL', res.outputPath)

  // 3. 解析输出
  const buf = fs.readFileSync(outPath)
  console.log('file size:', buf.length)
  const p = parseUltraHdr(buf)

  console.log('\n===== 主图像 XMP =====')
  console.log(p.primaryXmp ? '存在 (GContainer)' : '缺失!')
  if (p.primaryXmp) {
    console.log('  hdrgm:Version:', (p.primaryXmp.match(/hdrgm:Version="([^"]+)"/) || [])[1])
    console.log('  GainMap Item: ', p.primaryXmp.includes('Item:Semantic="GainMap"'))
    console.log('  Item:Length:  ', (p.primaryXmp.match(/Item:Length="(\d+)"/) || [])[1])
  }

  console.log('\n===== 主图像 ICC =====')
  if (p.icc) {
    const icc = parseIccInfo(p.icc)
    console.log('  size:', icc.size, 'class:', icc.class, 'space:', icc.colorSpace, 'pcs:', icc.pcs)
    console.log('  是否 sRGB(正确):', icc.colorSpace === 'RGB ' && icc.version >= 0x04000000)
  } else {
    console.log('  缺失!')
  }

  console.log('\n===== MPF =====')
  if (p.mpf) {
    const entries = parseMpf(p.mpf)
    console.log('  entries:', entries.length)
    entries.forEach((e, i) => console.log(`  [${i}] attr=${e.attr.toString(16)} size=${e.size} offset=${e.offset}`))
  } else {
    console.log('  缺失!')
  }

  console.log('\n===== 增益图 XMP (hdrgm) =====')
  if (p.gainMapXmp) {
    for (const k of ['hdrgm:Version', 'hdrgm:GainMapMin', 'hdrgm:GainMapMax', 'hdrgm:Gamma',
      'hdrgm:OffsetSDR', 'hdrgm:OffsetHDR', 'hdrgm:HDRCapacityMin', 'hdrgm:HDRCapacityMax',
      'hdrgm:BaseRenditionIsHDR']) {
      console.log('  ' + k + ':', (p.gainMapXmp.match(new RegExp(k + '="([^"]+)"')) || [])[1])
    }
  } else {
    console.log('  缺失!')
  }

  console.log('\n===== 增益图尺寸 =====')
  if (p.secondary) {
    // 尝试用 sharp 解出增益图（合成 SOI + body）
    try {
      const meta = await sharp(p.secondary).metadata()
      console.log('  增益图: ' + meta.width + 'x' + meta.height + ' channels=' + meta.channels)
    } catch (e) {
      console.log('  增益图解码失败:', e.message)
    }
  } else {
    console.log('  未找到次图像!')
  }

  // 4. 用 sharp 直接打开整个文件，确认 SDR 主图像可正常解码
  try {
    const meta = await sharp(outPath).metadata()
    console.log('\n===== 主图像 (sharp 解码) =====')
    console.log('  ' + meta.width + 'x' + meta.height, 'format=' + meta.format, 'channels=' + meta.channels)
  } catch (e) {
    console.log('\n主图像解码失败:', e.message)
  }
  stopBackend()
})().catch((e) => { console.error('验证失败:', e); stopBackend(); process.exit(1) })
