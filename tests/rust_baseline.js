/**
 * 生成 hdrconv 的 Kotlin 回归基准（存档对照用）：
 * ⚠️ Kotlin 已停止维护（archive/kotlin-backend/），本脚本对照已存档 jar 生成历史基准。
 *   1. 生成确定性 32x32 RGBA 渐变测试图（tests/rust_ref_input.png，纯 Node zlib，无依赖）
 *   2. 启动存档 Kotlin 后端（HDR_GPU_DISABLE=1 强制 CPU，保证与 Rust float64 可比）
 *   3. POST /convert（png 输出，参数与 hdrconv Settings::default() 完全一致）
 *   4. 保存 Kotlin 输出 tests/rust_ref_kotlin.png，关闭后端
 *
 * 用法：node tests/rust_baseline.js
 * 之后：cd backend/rust && cargo test -- --ignored
 */
const fs = require('fs')
const path = require('path')
const zlib = require('zlib')
const { ensureBackend, stopBackend, httpJson } = require('./backend_test_util')

// ---------------------------------------------------------------
// 最小 PNG 写入器（8-bit RGBA / 无滤波 / zlib），保证输入确定性
// ---------------------------------------------------------------
const CRC_TABLE = (() => {
  const t = new Int32Array(256)
  for (let n = 0; n < 256; n++) {
    let c = n
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1
    t[n] = c
  }
  return t
})()

function crc32(buf) {
  let c = -1
  for (const b of buf) c = CRC_TABLE[(c ^ b) & 0xff] ^ (c >>> 8)
  return (c ^ -1) >>> 0
}

function chunk(type, data) {
  const t = Buffer.from(type, 'ascii')
  const len = Buffer.alloc(4)
  len.writeUInt32BE(data.length)
  const crc = Buffer.alloc(4)
  crc.writeUInt32BE(crc32(Buffer.concat([t, data])))
  return Buffer.concat([len, t, data, crc])
}

function writePng(file, w, h, rgba) {
  const ihdr = Buffer.alloc(13)
  ihdr.writeUInt32BE(w, 0)
  ihdr.writeUInt32BE(h, 4)
  ihdr[8] = 8 // 位深
  ihdr[9] = 6 // 颜色类型 RGBA
  const stride = w * 4
  const raw = Buffer.alloc((stride + 1) * h)
  for (let y = 0; y < h; y++) {
    raw[y * (stride + 1)] = 0 // 滤波类型 None
    rgba.copy(raw, y * (stride + 1) + 1, y * stride, (y + 1) * stride)
  }
  const idat = zlib.deflateSync(raw)
  const png = Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', ihdr),
    chunk('IDAT', idat),
    chunk('IEND', Buffer.alloc(0)),
  ])
  fs.writeFileSync(file, png)
}

// ---------------------------------------------------------------
// 确定性渐变输入：覆盖暗部/中间调/高光/极值，练出各类 round/钳制路径
// ---------------------------------------------------------------
const W = 32
const H = 32

function makeGradient() {
  const rgba = Buffer.alloc(W * H * 4)
  let i = 0
  for (let y = 0; y < H; y++) {
    for (let x = 0; x < W; x++) {
      rgba[i++] = Math.round((x * 255) / (W - 1))
      rgba[i++] = Math.round((y * 255) / (H - 1))
      rgba[i++] = Math.round(((x + y) * 255) / (W + H - 2))
      rgba[i++] = 255
    }
  }
  // 极值像素
  const first = 0
  const last = (W * H - 1) * 4
  rgba[first] = 0
  rgba[first + 1] = 0
  rgba[first + 2] = 0
  rgba[last] = 255
  rgba[last + 1] = 255
  rgba[last + 2] = 255
  return rgba
}

// ---------------------------------------------------------------
async function main() {
  process.env.HDR_GPU_DISABLE = '1' // 强制 Kotlin 走 CPU float64，与 Rust 一致
  const input = path.join(__dirname, 'rust_ref_input.png')
  const kotlinOut = path.join(__dirname, 'rust_ref_kotlin.png')
  fs.rmSync(kotlinOut, { force: true })

  writePng(input, W, H, makeGradient())
  console.log('测试输入已生成:', input, `(${W}x${H})`)

  await ensureBackend()
  try {
    const res = await httpJson('POST', '/convert', {
      inputPath: input,
      outputPath: 'rust_ref_kotlin.png',
      settings: {
        outputFormat: 'png',
        peakNits: 574, // 与 hdrconv Settings::default() 一致
        whiteNits: 203,
        gamma: 0.9,
        // rgbAdjustment 省略 → Kotlin 默认 0.96/1.0/1.0，与 Rust RgbAdjustment::default() 一致
      },
    })
    if (!res.success) throw new Error('Kotlin 转换失败: ' + (res.message || ''))
    if (!fs.existsSync(kotlinOut)) throw new Error('Kotlin 输出未生成: ' + kotlinOut)
    console.log('基准已生成:', kotlinOut, '| 检测色彩空间:', res.detectedColorSpace)

    // Ultra HDR 基准（outputFormat "jpg" = Kotlin 语义的增益图链路）：
    // hdrIntensity = log2(峰值/白点) = log2(574/203)，与 Rust settings.ev() 一致，
    // 保证 maxBoost 相同 → XMP 增益图统计量可比
    const uhdrOut = path.join(__dirname, 'rust_ref_kotlin_uhdr.jpg')
    fs.rmSync(uhdrOut, { force: true })
    const res2 = await httpJson('POST', '/convert', {
      inputPath: input,
      outputPath: 'rust_ref_kotlin_uhdr.jpg',
      settings: {
        outputFormat: 'jpg', // Kotlin: jpg = Ultra HDR
        peakNits: 574,
        whiteNits: 203,
        gamma: 0.9,
        hdrIntensity: Math.log2(574 / 203),
      },
    })
    if (!res2.success) throw new Error('Kotlin ultra-hdr 转换失败: ' + (res2.message || ''))
    if (!fs.existsSync(uhdrOut)) throw new Error('Kotlin ultra-hdr 输出未生成: ' + uhdrOut)
    console.log('uhdr 基准已生成:', uhdrOut)

    console.log('下一步: cd backend/rust && cargo test -- --ignored')
  } finally {
    stopBackend()
  }
}

main().catch((e) => {
  console.error(e)
  process.exit(1)
})