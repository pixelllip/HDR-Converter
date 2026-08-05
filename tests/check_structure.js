/** 检查单个文件的 Ultra HDR 结构是否符合真实 Google 格式 */
const fs = require('fs')

function check(file) {
  const buf = fs.readFileSync(file)
  let off = 2
  const segs = []
  let hasJfif = false
  let sofPrimary = '-'
  while (off + 4 <= buf.length) {
    if (buf[off] !== 0xFF) break
    const m = buf.readUInt16BE(off)
    if (m === 0xFFDA) { segs.push('SOS'); break }
    if (m === 0xFFD9) break
    const l = buf.readUInt16BE(off + 2)
    const d = buf.slice(off + 4, off + 2 + l)
    let de = ''
    if (m === 0xFFE0) { de = 'JFIF'; hasJfif = true }
    if (m === 0xFFE1 && d.toString('latin1', 0, 28) === 'http://ns.adobe.com/xap/1.0/') de = 'XMP'
    if (m === 0xFFE2 && d.toString('latin1', 0, 12) === 'ICC_PROFILE\0') de = 'ICC'
    if (m === 0xFFE2 && d.toString('latin1', 0, 4) === 'MPF\0') {
      de = 'MPF(payload=' + (l - 2) + ')'
      // 检查 MPF 是否含 0x002A 幻数
      const hasMagic = d[6] === 0x00 && d[7] === 0x2A
      const verTag = d.readUInt16BE(14).toString(16)
      de += (hasMagic ? ' [OK magic]' : ' [缺0x002A!]') + ' verTag=0x' + verTag
    }
    if (m >= 0xFFC0 && m <= 0xFFC2 && m !== 0xFFC4) { sofPrimary = '0x' + m.toString(16).toUpperCase() + '(Nf=' + buf[off + 9] + ')'; de = de || 'SOF' }
    segs.push(m.toString(16).toUpperCase() + (de ? '(' + de + ')' : ''))
    off += 2 + l
  }

  // 增益图
  let p = off + 4 + buf.readUInt16BE(off + 2)
  let eoi = -1
  for (let i = p; i < buf.length - 1; i++) {
    if (buf[i] === 0xFF && buf[i + 1] === 0x00) { i++; continue }
    if (buf[i] === 0xFF && buf[i + 1] === 0xD9) { eoi = i + 2; break }
  }
  const sec = buf.slice(eoi)
  let secInfo = '无次图像'
  if (sec.length > 4) {
    const first4 = [...sec.slice(0, 4)].map((x) => x.toString(16).padStart(2, '0')).join(' ')
    let nf = -1, sofG = '-', hasXmp = false, so = 2
    while (so + 4 <= sec.length) {
      if (sec[so] !== 0xFF) break
      const m = sec.readUInt16BE(so)
      const l = sec.readUInt16BE(so + 2)
      if (m === 0xFFE1 && sec.toString('latin1', so + 4, so + 4 + 28) === 'http://ns.adobe.com/xap/1.0/') hasXmp = true
      if (m >= 0xFFC0 && m <= 0xFFCF && m !== 0xFFC4 && m !== 0xFFC8 && m !== 0xFFCC) { nf = sec[so + 9]; sofG = '0x' + m.toString(16).toUpperCase(); break }
      so += 2 + l
    }
    secInfo = `首4字节=${first4} SOF=${sofG} Nf=${nf} XMP=${hasXmp}`
  }

  return {
    segments: segs.join(' | '),
    jfif: hasJfif,
    sofPrimary,
    secondary: secInfo,
    size: buf.length,
  }
}

const file = process.argv[2]
if (!file) { console.error('usage: node check_structure.js <file>'); process.exit(1) }
const r = check(file)
console.log('文件:', file, '大小:', r.size)
console.log('主图像:', r.segments)
console.log('  JFIF:', r.jfif, ' SOF0:', r.sofPrimary)
console.log('增益图:', r.secondary)
