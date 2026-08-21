/**
 * EXIF Orientation 端到端验收：调用 Kotlin 后端 /preview，验证输出窗口的预览
 * 尺寸按 EXIF orientation 正确旋转与缩放。
 *
 * 用 sharp 生成两张存储像素相同、但 EXIF orientation 不同的 JPEG：
 *   orient_1.jpg : 100x60  存储像素＝存储像素，orientation=1   （正放）
 *   orient_6.jpg : 100x60  存储像素＝存储像素，orientation=6   （90° CW，物理仍是 100x60）
 *
 * 期望：
 *   /preview({ orient_1 })
 *     width=50, height=30   （100x60 × 0.5）
 *   /preview({ orient_6 })
 *     width=30, height=50   （orientation=6 后 60x100 × 0.5）
 * 修复前两者都是 50x30（后端忽略 orientation）。
 */
'use strict'

const fs = require('fs')
const os = require('os')
const path = require('path')
const sharp = require('sharp')
const { ensureBackend, stopBackend, httpJson, convertImage } = require('./backend_test_util')

const TMP = path.join(os.tmpdir(), 'hdr_exif_e2e_test_' + process.pid)
fs.mkdirSync(TMP, { recursive: true })

const STORED_W = 100
const STORED_H = 60

async function makeOrientedJpeg(orientation) {
  // 用 4 块色彩画一张「可肉眼分辨顶部/底部」的图：左半 RGBA=RGB(220,40,40) 右半=RGB(40,40,220)
  const svg = Buffer.from(
    `<svg width="${STORED_W}" height="${STORED_H}" xmlns="http://www.w3.org/2000/svg">` +
      `<rect width="100%" height="100%" fill="#fff"/>` +
      `<rect width="50%" height="100%" fill="rgb(220,40,40)"/>` +
      `<text x="50%" y="50%" font-size="${Math.max(8, Math.floor(STORED_H / 6))}" font-family="Arial" fill="#fff" text-anchor="middle" dominant-baseline="middle">TOP-TXT</text>` +
      `<text x="50%" y="92%" font-size="${Math.max(8, Math.floor(STORED_H / 6))}" font-family="Arial" fill="#000" text-anchor="middle">BOT-TXT</text>` +
    `</svg>`
  )
  const out = path.join(TMP, `e2e_orient_${orientation}.jpg`)
  await sharp(svg).jpeg({ quality: 90 }).withMetadata({ orientation }).toFile(out)
  return out
}

async function preview(inputPath) {
  return await httpJson('POST', '/preview', {
    inputPath,
    settings: {
      hdrIntensity: 0.5,
      gamma: 1.0,
      outputFormat: 'jpg',
      quality: 0.9,
      rgbAdjustment: { red: 1.0, green: 1.0, blue: 1.0 },
      whiteNits: 203,
      peakNits: 600
    }
  })
}

function assertEq(actual, expected, label) {
  const ok = actual === expected
  console.log(`  ${label}: actual=${actual} expected=${expected}  ${ok ? 'OK' : 'FAIL'}`)
  if (!ok) process.exitCode = 1
}

async function main() {
  console.log(`工作目录: ${TMP}`)
  const o1 = await makeOrientedJpeg(1)
  const o6 = await makeOrientedJpeg(6)
  const o8 = await makeOrientedJpeg(8)
  console.log('生成测试图片（存储像素一致，orientation 不同）:')
  console.log('  ', o1, '(orientation=1)')
  console.log('  ', o6, '(orientation=6)')
  console.log('  ', o8, '(orientation=8)')

  console.log('\n拉起 Kotlin 后端…')
  await ensureBackend()

  try {
    console.log('\n--- orientation=1 (期望 width=50, height=30) ---')
    const r1 = await preview(o1)
    assertEq(r1.width, 50, 'width')
    assertEq(r1.height, 30, 'height')

    console.log('\n--- orientation=6 (期望 width=30, height=50  修复前 50x30) ---')
    const r6 = await preview(o6)
    assertEq(r6.width, 30, 'width')
    assertEq(r6.height, 50, 'height')

    console.log('\n--- orientation=8 (期望 width=30, height=50  修复前 50x30) ---')
    const r8 = await preview(o8)
    assertEq(r8.width, 30, 'width')
    assertEq(r8.height, 50, 'height')

    // 保存预览返回的 data URL 看下（可肉眼分辨朝向）
    if (r6 && r6.dataUrl) {
      const m = /^data:image\/(\w+);base64,(.+)$/.exec(r6.dataUrl)
      if (m) {
        const ext = m[1] === 'jpeg' ? 'jpg' : m[1]
        fs.writeFileSync(path.join(TMP, `e2e_orient_6_preview.${ext}`), Buffer.from(m[2], 'base64'))
        console.log(`\norientation=6 预览产物: ${path.join(TMP, `e2e_orient_6_preview.${ext}`)}`)
      }
    }

    // /convert 实测：原 orient=6 的图转换后文件也应已转正（不再侧倒）
    console.log('\n--- 整图保存 /convert 验证（修复前图片侧倒，修复后正向） ---')
    const out6 = path.join(TMP, 'e2e_orient_6_converted.png')
    await convertImage({
      inputPath: o6,
      outputPath: out6,
      settings: {
        hdrIntensity: 0.5,
        gamma: 1.0,
        outputFormat: 'png'
      }
    })
    const convertedMeta = await sharp(out6).metadata()
    console.log(`  转换文件 ${out6}: ${convertedMeta.width}x${convertedMeta.height}`)
    assertEq(convertedMeta.width, 60, 'width (期望 60)')
    assertEq(convertedMeta.height, 100, 'height (期望 100)')
  } finally {
    console.log('\n关闭后端…')
    stopBackend()
  }

  console.log(process.exitCode ? '\nFAIL' : '\nALL PASS')
}

main().catch((e) => { console.error(e); stopBackend(); process.exit(1) })
