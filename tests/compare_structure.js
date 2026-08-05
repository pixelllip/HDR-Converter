/** 对比我们的输出与真实 Google Ultra HDR 文件的结构 */
const fs = require('fs')
const sharp = require('sharp')
const { convertImage, stopBackend } = require('./backend_test_util')

function structure(file) {
  const buf = fs.readFileSync(file)
  let off = 2
  const segs = []
  while (off + 4 <= buf.length) {
    if (buf[off] !== 0xFF) break
    const m = buf.readUInt16BE(off)
    if (m === 0xFFDA) { segs.push('SOS'); break }
    if (m === 0xFFD9) break
    const l = buf.readUInt16BE(off + 2)
    const d = buf.slice(off + 4, off + 2 + l)
    let de = ''
    if (m === 0xFFE0) de = 'JFIF'
    if (m === 0xFFE1 && d.toString('latin1', 0, 28) === 'http://ns.adobe.com/xap/1.0/') de = 'XMP'
    if (m === 0xFFE2 && d.toString('latin1', 0, 12) === 'ICC_PROFILE\0') de = 'ICC'
    if (m === 0xFFE2 && d.toString('latin1', 0, 4) === 'MPF\0') de = 'MPF(payload=' + (l - 2) + ')'
    if (m === 0xFFE2 && d.toString('latin1', 0, 28).startsWith('urn:iso:std:iso:ts:21496')) de = 'ISO21496'
    if (m >= 0xFFC0 && m <= 0xFFC2 && m !== 0xFFC4) de = 'SOF' + String(m - 0xFFC0) + '(Nf=' + buf[off + 9] + ')'
    if (m === 0xFFDB) de = 'DQT'
    if (m === 0xFFC4) de = 'DHT'
    segs.push(m.toString(16).toUpperCase() + (de ? '(' + de + ')' : ''))
    off += 2 + l
  }
  return segs.join(' | ')
}

function gainMapInfo(file) {
  const buf = fs.readFileSync(file)
  let off = 2
  while (off + 4 <= buf.length) {
    if (buf[off] !== 0xFF) break
    const m = buf.readUInt16BE(off)
    if (m === 0xFFDA) break
    const l = buf.readUInt16BE(off + 2)
    off += 2 + l
  }
  let p = off + 4 + buf.readUInt16BE(off + 2)
  let eoi = -1
  for (let i = p; i < buf.length - 1; i++) {
    if (buf[i] === 0xFF && buf[i + 1] === 0x00) { i++; continue }
    if (buf[i] === 0xFF && buf[i + 1] === 0xD9) { eoi = i + 2; break }
  }
  const sec = buf.slice(eoi)
  let nf = -1
  let m0 = 0
  let hasXmp = false
  let so = 2
  while (so + 4 <= sec.length) {
    if (sec[so] !== 0xFF) break
    const m = sec.readUInt16BE(so)
    const l = sec.readUInt16BE(so + 2)
    if (m === 0xFFE1 && sec.toString('latin1', so + 4, so + 4 + 28) === 'http://ns.adobe.com/xap/1.0/') hasXmp = true
    if (m >= 0xFFC0 && m <= 0xFFCF && m !== 0xFFC4 && m !== 0xFFC8 && m !== 0xFFCC) {
      nf = sec[so + 9]; m0 = m; break
    }
    so += 2 + l
  }
  return { nf, sof: '0x' + m0.toString(16).toUpperCase(), hasXmp }
}

;(async () => {
  const input = 'tmp_cmp_input.png'
  await sharp({
    create: { width: 640, height: 360, channels: 3, background: { r: 20, g: 60, b: 120 } }
  }).composite([{
    input: Buffer.from(
      '<svg width="640" height="360"><defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#101010"/><stop offset="1" stop-color="#ffffff"/></linearGradient></defs><rect width="640" height="360" fill="url(#g)"/><circle cx="500" cy="80" r="60" fill="#ffffff"/></svg>'
    )
  }]).png().toFile(input)

  await convertImage({
    inputPath: input,
    outputPath: 'tmp_cmp_out.jpg',
    settings: { hdrIntensity: 2.0, fineTuneBrightness: 0.5, gamma: 0.9, outputFormat: 'jpg' }
  })

  console.log('=== 主图像段结构 ===')
  console.log('REAL:', structure('real_sample.jpg'))
  console.log('OURS:', structure('tmp_cmp_out.jpg'))
  console.log('\n=== 增益图 ===')
  console.log('REAL:', JSON.stringify(gainMapInfo('real_sample.jpg')))
  console.log('OURS:', JSON.stringify(gainMapInfo('tmp_cmp_out.jpg')))
  stopBackend()
})().catch((e) => { console.error(e); stopBackend(); process.exit(1) })
