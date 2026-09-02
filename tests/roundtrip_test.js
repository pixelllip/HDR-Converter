/**
 * 端到端闭环验证：
 *  1. SDR 输入 -> convertImage -> Ultra HDR JPEG
 *  2. detectAndExtractGainMap 提取增益图 + hdrgm 元数据
 *  3. 按规范公式解码，验证重建 HDR 在亮部高于 SDR（增益图生效）
 */
const fs = require('fs')
const path = require('path')
const sharp = require('sharp')
const { convertImage, stopBackend } = require('./backend_test_util')
const { detectAndExtractGainMap } = require('./hdr-gainmap')

function srgbToLinear(v) { return v <= 0.04045 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4) }

; (async () => {
  const inputPath = path.join(__dirname, 'tmp_rt_input.png')
  // 制造一张有暗部、中间调、高光的图
  await sharp({
    create: { width: 320, height: 200, channels: 3, background: { r: 30, g: 30, b: 30 } }
  }).composite([
    { input: Buffer.from('<svg width="320" height="200"><rect width="160" height="200" fill="#808080"/><rect x="160" width="160" height="200" fill="#ffffff"/></svg>') }
  ]).png().toFile(inputPath)

  const outPath = path.join(__dirname, 'tmp_rt_output.jpg')
  await convertImage({
    inputPath,
    outputPath: outPath,
    settings: { hdrIntensity: 4.0, fineTuneBrightness: 1.0, gamma: 0.9, outputFormat: 'jpg' }
  })

  const fileBuf = fs.readFileSync(outPath)
  const result = detectAndExtractGainMap(fileBuf, '.jpg')
  if (!result || !result.hasGainMap) {
    console.error('❌ 未能提取增益图')
    process.exit(1)
  }
  console.log('✅ 提取到增益图', `${result.gainMapWidth}x${result.gainMapHeight}`)
  console.log('   hdrgm 元数据:', JSON.stringify(result.metadata, null, 2))

  // 解码 SDR 主图像（sharp 只解第一张 JPEG = 主图像）
  const sdrRaw = await sharp(fileBuf).raw().toBuffer({ resolveWithObject: true })
  const { width, height } = sdrRaw.info
  const sdr = sdrRaw.data

  // 解码增益图
  const gmBuf = Buffer.from(result.gainMapBase64, 'base64')
  const gmRaw = await sharp(gmBuf).raw().toBuffer({ resolveWithObject: true })
  const gmd = gmRaw.data
  const gw = gmRaw.info.width, gh = gmRaw.info.height

  // 按规范公式重建 HDR（模拟 viewer）
  const m = result.metadata
  const gainMapMin = m.gainMapMinLog2
  const gainMapMax = m.gainMapMaxLog2
  const gamma = m.gamma
  const offS = m.offsetSdr
  const offH = m.offsetHdr
  const maxDisplayBoost = Math.pow(2, m.hdrCapacityMaxLog2)
  const weight = 1.0 // 满提升

  // 统计亮部（SDR 最亮像素）的重建值
  let maxSdrLin = 0
  let maxHdrLin = 0
  let brightCount = 0
  const samples = []
  const sx = width / gw, sy = height / gh
  for (let y = 0; y < height; y += 4) {
    for (let x = 0; x < width; x += 4) {
      const i = (y * width + x) * 3
      const sdrR = srgbToLinear(sdr[i] / 255)
      const sdrG = srgbToLinear(sdr[i + 1] / 255)
      const sdrB = srgbToLinear(sdr[i + 2] / 255)
      const ySdr = 0.2126 * sdrR + 0.7152 * sdrG + 0.0722 * sdrB

      const gx = Math.min(Math.floor(x / sx), gw - 1)
      const gy = Math.min(Math.floor(y / sy), gh - 1)
      const recovery = (0.2126 * gmd[(gy * gw + gx) * 3] + 0.7152 * gmd[(gy * gw + gx) * 3 + 1] + 0.0722 * gmd[(gy * gw + gx) * 3 + 2]) / 255
      const logRecovery = Math.pow(recovery, 1 / gamma)
      const logBoost = gainMapMin * (1 - logRecovery) + gainMapMax * logRecovery
      const gainFactor = Math.pow(2, logBoost * weight)

      const hdrR = (sdrR + offS) * gainFactor - offH
      if (ySdr > 0.8) {
        brightCount++
        maxSdrLin = Math.max(maxSdrLin, ySdr)
        maxHdrLin = Math.max(maxHdrLin, 0.2126 * hdrR + 0.7152 * hdrR + 0.0722 * hdrR)
      }
      if (samples.length < 8 && x < 40) {
        samples.push({ x, y, sdrLum: +ySdr.toFixed(3), recovery: +recovery.toFixed(3), hdrFactor: +gainFactor.toFixed(3) })
      }
    }
  }

  console.log('\n===== 重建结果（满显示提升 weight=1）=====')
  console.log('亮部像素数:', brightCount)
  console.log('亮部最大 SDR 线性亮度:', maxSdrLin.toFixed(4))
  console.log('亮部最大重建 HDR 线性亮度:', maxHdrLin.toFixed(4))
  console.log('HDR/SDR 峰值比:', (maxHdrLin / Math.max(maxSdrLin, 1e-6)).toFixed(3))

  // 判定：亮部 HDR 峰值应明显高于 SDR（>1.0，因为 maxBoost=4）
  const ok = maxHdrLin > 1.0 && maxHdrLin / Math.max(maxSdrLin, 1e-6) > 1.5
  console.log('\n亮部重建 HDR > SDR 白点:', ok ? '✅ 通过' : '❌ 未通过')

  // 打印部分采样
  console.log('\n采样（左半暗部+中间调）:')
  samples.forEach((s) => console.log(`  x=${s.x} y=${s.y} sdrLum=${s.sdrLum} recovery=${s.recovery} hdrFactor=${s.hdrFactor}`))
  stopBackend()
})().catch((e) => { console.error('❌ 验证失败:', e); stopBackend(); process.exit(1) })
