/**
 * 验证图片 png/jpg_icc 输出为 Rec.2020/PQ（2026-08-13）
 *
 * 旧 bug：图片像素是 sRGB 编码（applyHdrTransform 末尾 linearToSrgb）却标 Rec.2020/PQ ICC → 色彩错乱。
 * 修复：applyHdrTransformToRec2020Pq 末尾改为 Rec.709→Rec.2020→PQ，像素与 ICC 一致。
 *
 * 断言：纯蓝/纯红经 /convert jpg_icc 后解码的像素 ≈ 本地用同一公式算出的 Rec.2020/PQ 码
 *       （绝不是旧 sRGB 值，如纯蓝旧值 [0,0,255]）。
 */
const fs = require('fs')
const path = require('path')
const { spawnSync } = require('child_process')
const { ensureBackend, stopBackend, httpJson } = require('./backend_test_util')

const FFMPEG = path.join(__dirname, '..', 'backend', 'ffmpeg', 'ffmpeg.exe')
const TMP = path.join(__dirname, 'tmp_video_hdr')
const BLUE = path.join(TMP, 'ic_blue.png')
const RED = path.join(TMP, 'ic_red.png')

if (!fs.existsSync(BLUE)) spawnSync(FFMPEG, ['-y', '-f', 'lavfi', '-i', 'color=c=0x0000ff:size=64x64:rate=1', '-frames:v', '1', BLUE], { windowsHide: true })
if (!fs.existsSync(RED)) spawnSync(FFMPEG, ['-y', '-f', 'lavfi', '-i', 'color=c=0xff0000:size=64x64:rate=1', '-frames:v', '1', RED], { windowsHide: true })

function srgbToLinear(v) { return v <= 0.04045 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4) }
function pqEncode(l) {
  const ll = Math.max(0, Math.min(1, l))
  const m1 = 0.1593017578125, m2 = 78.84375, c1 = 0.8359375, c2 = 18.8515625, c3 = 18.6875
  return Math.pow((c1 + c2 * Math.pow(ll, m1)) / (1 + c3 * Math.pow(ll, m1)), m2)
}

/** 复刻后端 applyHdrTransformToRec2020Pq 对纯色帧的预期 Rec.2020/PQ 码 */
function expected(rgb255, gamma, exposure, fineTune) {
  const r0 = srgbToLinear(rgb255[0] / 255), g0 = srgbToLinear(rgb255[1] / 255), b0 = srgbToLinear(rgb255[2] / 255)
  const mean = 0.2126 * r0 + 0.7152 * g0 + 0.0722 * b0
  let autoGamma = 1.0
  if (mean > 0.001 && mean < 0.999) autoGamma = Math.max(0.3, Math.min(3.0, Math.log(0.5) / Math.log(mean)))
  const ag = (v) => Math.pow(Math.max(v, 0), autoGamma)
  const ar = ag(r0) * exposure * fineTune, ag_ = ag(g0) * exposure * fineTune, ab = ag(b0) * exposure * fineTune
  const pr = Math.pow(Math.max(ar, 0), gamma), pg = Math.pow(Math.max(ag_, 0), gamma), pb = Math.pow(Math.max(ab, 0), gamma)
  const r2020 = 0.6274038959 * pr + 0.3292830384 * pg + 0.0433130642 * pb
  const g2020 = 0.0690972894 * pr + 0.9195403951 * pg + 0.0113623156 * pb
  const b2020 = 0.0163914389 * pr + 0.0880133078 * pg + 0.8955952528 * pb
  const scale = 203 / 10000
  return [pqEncode(r2020 * scale) * 255, pqEncode(g2020 * scale) * 255, pqEncode(b2020 * scale) * 255]
}

function decodeJpegPix(jpegBuf) {
  const tmp = path.join(TMP, 'ic_out.jpg')
  fs.writeFileSync(tmp, jpegBuf)
  const out = spawnSync(FFMPEG, ['-v', 'error', '-i', tmp, '-f', 'rawvideo', '-pix_fmt', 'rgb24', '-'], { maxBuffer: 1 << 28, windowsHide: true })
  return [out.stdout[0], out.stdout[1], out.stdout[2]]
}

async function main() {
  const port = await ensureBackend()
  // 曝光 = 峰值/白点 = 2^EV（2026-08-13：微调明暗已移除，与视频预览一致）
  const exposure = 574 / 203
  const settings = { hdrIntensity: 1.5, gamma: 0.9, outputFormat: 'jpg_icc', whiteNits: 203, peakNits: 574, quality: 0.95 }
  const cases = [
    ['纯蓝 (0,0,255)', BLUE, [0, 0, 255]],
    ['纯红 (255,0,0)', RED, [255, 0, 0]]
  ]
  let allOk = true
  for (const [name, frame, rgb] of cases) {
    const out = path.join(TMP, 'ic_' + name[1] + '.jpg')
    const r = await httpJson('POST', '/convert', { inputPath: frame, outputPath: out, settings })
    if (!r.success) throw new Error(r.message)
    const actual = decodeJpegPix(fs.readFileSync(out))
    const exp = expected(rgb, 0.9, exposure, 1.0)
    const d = Math.max(Math.abs(actual[0] - exp[0]), Math.abs(actual[1] - exp[1]), Math.abs(actual[2] - exp[2]))
    // 旧 sRGB 值（纯蓝 [0,0,255]、纯红 [255,0,0]）与新 PQ 码差异应很大
    const ok = d <= 14
    if (!ok) allOk = false
    console.log(`${ok ? '✅' : '❌'} ${name}: 实际=${actual.join(',')} 预期Rec2020PQ=${exp.map(v => Math.round(v)).join(',')} 差=${d.toFixed(1)}（旧sRGB=${rgb.join(',')}）`)
  }
  stopBackend()
  console.log(allOk ? '\n✅ 图片 jpg_icc 已为 Rec.2020/PQ' : '\n❌ 不匹配')
  process.exit(allOk ? 0 : 1)
}
main().catch((e) => { console.error('❌', e.message); stopBackend(); process.exit(1) })
