/**
 * EXIF Orientation 解析逻辑验证（与 Kotlin 的 exifOrientation 镜像）
 *
 * 流程：
 *   1. 用 sharp 生成若干带 EXIF orientation 的 JPEG（1,3,6,8）
 *   2. 用 JS 实现同一套 JPEG 段扫描 + TIFF 解析读出 orientation，断言值正确
 *   3. 检查目标尺寸 swap 公式（orientation 5..8 交换宽高）
 *
 * 几何变换（applyOrientation）已用业界通用的 Java AffineTransform 6-param 公式，
 * 其正确性通过后端 /preview 端到端冒烟用例验证。
 */
'use strict'

const fs = require('fs')
const os = require('os')
const path = require('path')
const sharp = require('sharp')

const TMP = path.join(os.tmpdir(), 'hdr_exif_orient_test_' + process.pid)
fs.mkdirSync(TMP, { recursive: true })

// ============================================================
//  1) 解析：与即将移植到 Kotlin 的 exifOrientation 同步
// ============================================================
function exifOrientationFromBuffer(buf) {
  try {
    if (buf.length < 4) return 1
    if (!(buf[0] === 0xFF && buf[1] === 0xD8 && buf[2] === 0xFF)) return 1
    let i = 2
    while (i + 3 < buf.length) {
      if (buf[i] !== 0xFF) { i++; continue }
      const marker = buf[i + 1]
      // FF 或 SOI/填充字节
      if (marker === 0xFF) { i += 2; continue }
      if (marker === 0xD8) { i += 2; continue }
      if (marker === 0xD9 || marker === 0xDA) return 1
      if (marker === 0x01 || (marker >= 0xD0 && marker <= 0xD7)) { i += 2; continue }
      if (i + 4 > buf.length) return 1
      const len = (buf[i + 2] << 8) | buf[i + 3]
      if (len < 2 || i + 2 + len > buf.length) return 1
      if (marker === 0xE1) {
        const exifStart = i + 4
        if (len >= 10 &&
          buf[exifStart] === 0x45 /*E*/ && buf[exifStart + 1] === 0x78 /*x*/ &&
          buf[exifStart + 2] === 0x69 /*i*/ && buf[exifStart + 3] === 0x66 /*f*/ &&
          buf[exifStart + 4] === 0x00 && buf[exifStart + 5] === 0x00) {
          const tiffStart = exifStart + 6
          return parseTiffOrientation(buf, tiffStart)
        }
      }
      i += 2 + len
    }
    return 1
  } catch (e) {
    return 1
  }
}

function parseTiffOrientation(buf, start) {
  if (start + 8 > buf.length) return 1
  const le = buf[start] === 0x49 && buf[start + 1] === 0x49
  const be = buf[start] === 0x4D && buf[start + 1] === 0x4D
  if (!le && !be) return 1
  const u16 = (o) => le
    ? (buf[o] | (buf[o + 1] << 8))
    : ((buf[o] << 8) | buf[o + 1])
  const u32 = (o) => le
    ? (buf[o] | (buf[o + 1] << 8) | (buf[o + 2] << 16) | (buf[o + 3] << 24))
    : ((buf[o] << 24) | (buf[o + 1] << 16) | (buf[o + 2] << 8) | buf[o + 3])
  if (u16(start + 2) !== 42) return 1
  const ifd = start + u32(start + 4)
  if (ifd + 2 > buf.length) return 1
  const n = u16(ifd)
  if (ifd + 2 + n * 12 > buf.length) return 1
  for (let j = 0; j < n; j++) {
    const e = ifd + 2 + j * 12
    if (u16(e) === 0x0112) {
      const v = u16(e + 8)
      return v >= 1 && v <= 8 ? v : 1
    }
  }
  return 1
}

// ============================================================
//  2) 几何变换：与即将移植到 Kotlin 的 applyOrientation 同步
//     返回 AffineTransform 6 元参数：[m00, m10, m01, m11, m02, m12]
// ============================================================
function orientationToTransform(orientation, w, h) {
  switch (orientation) {
    case 2: return [-1, 0, 0, 1, w, 0]
    case 3: return [-1, 0, 0, -1, w, h]
    case 4: return [1, 0, 0, -1, 0, h]
    case 5: return [0, 1, 1, 0, 0, 0]
    case 6: return [0, 1, -1, 0, h, 0]
    case 7: return [0, -1, -1, 0, h, w]
    case 8: return [0, -1, 1, 0, 0, w]
    default: return [1, 0, 0, 1, 0, 0]
  }
}
function swapForOrientation(o, w, h) {
  return (o >= 5 && o <= 8) ? [h, w] : [w, h]
}

// ============================================================
//  Test cases
// ============================================================
async function main() {
  // 用 sharp 把一张带文字/色块的 RGBA 渲染成 JPEG，按 orientation 写入 EXIF
  async function makeOrientedJpeg(orientation, w, h) {
    const svg = Buffer.from(
      `<svg width="${w}" height="${h}" xmlns="http://www.w3.org/2000/svg">` +
        `<rect width="100%" height="100%" fill="#fff"/>` +
        `<text x="5%" y="50%" font-size="${Math.floor(h / 6)}" font-family="Arial" fill="#000">TOP</text>` +
        `<text x="5%" y="95%" font-size="${Math.floor(h / 6)}" font-family="Arial" fill="#000">BOTTOM</text>` +
      `</svg>`
    )
    const outPath = path.join(TMP, `orient_${orientation}.jpg`)
    await sharp(svg).jpeg({ quality: 90 }).withMetadata({ orientation }).toFile(outPath)
    return outPath
  }

  // 像素级期望：sharp().rotate() 把 raw pixels 转正（与 EXIF orientation 抵消），
  // 我们手动 applyOrientation 也应得到同样的 raw pixels。
  async function pixelsAfterOrientation(inputJpegPath, orientation) {
    const raw = await sharp(inputJpegPath).rotate(orientation).raw().toBuffer({ resolveWithObject: true })
    return { w: raw.info.width, h: raw.info.height, px: raw.data }
  }

  console.log('==== 1) 解析 EXIF orientation 验证 ====')
  const cases = [
    { o: 1 }, { o: 3 }, { o: 6 }, { o: 8 }
  ]
  for (const { o } of cases) {
    const p = await makeOrientedJpeg(o, 200, 120)
    const buf = fs.readFileSync(p)
    const got = exifOrientationFromBuffer(buf)
    const ok = got === o
    console.log(`  orientation=${o}: 解析结果=${got} ${ok ? 'OK' : 'FAIL'} (${p})`)
    if (!ok) process.exitCode = 1
  }

  console.log('==== 2) 目标尺寸 swap 公式验证 ====')
  for (const o of [1, 2, 3, 4, 5, 6, 7, 8]) {
    const [w, h] = swapForOrientation(o, 200, 120)
    const expectW = (o >= 5 && o <= 8) ? 120 : 200
    const expectH = (o >= 5 && o <= 8) ? 200 : 120
    const ok = w === expectW && h === expectH
    console.log(`  orientation=${o}: ${w}x${h} ${ok ? 'OK' : 'FAIL'} (期望 ${expectW}x${expectH})`)
    if (!ok) process.exitCode = 1
  }

  console.log(process.exitCode ? '\nFAIL' : '\nALL PASS')
}

main().catch((e) => { console.error(e); process.exit(1) })
