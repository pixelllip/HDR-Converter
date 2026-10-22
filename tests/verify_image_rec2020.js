/**
 * 验证图片 png/jpg_icc 输出为所选输出色域 + PQ（2026-10：明度 B 曲线）
 *
 * 旧 bug：图片像素是 sRGB 编码却标 Rec.2020/PQ ICC → 色彩错乱。
 * 现行链路：sRGB→线性→明度 B 曲线（中调抬升 + 白点滚降）→ BT.709→输出色域 → PQ。
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

// 与 Rust ultra_hdr::tone_target 同式（B 方案：中调抬升 + 白点 rolloff 滚降）
const PEAK_Y = 0.30
const PEAK_FRAC = 0.80
function smoothstep(a, b, x) {
  const t = Math.max(0, Math.min(1, (x - a) / (b - a)))
  return t * t * (3 - 2 * t)
}
function toneTarget(y, m, rolloff) {
  m = Math.max(m, 1)
  const r = Math.max(0, Math.min(1, rolloff))
  const tPeak = PEAK_Y * (1 + (m - 1) * PEAK_FRAC)
  if (y <= PEAK_Y) return y * (1 + (m - 1) * PEAK_FRAC * smoothstep(0, PEAK_Y, y))
  return tPeak + ((1 + (m - 1) * r) - tPeak) * smoothstep(PEAK_Y, 1, y)
}

/** 复刻后端 apply_hdr_rec2020_pq 对纯色帧的预期 Rec.2020/PQ 码 */
function expected(rgb255, exposure, rolloff) {
  const r0 = srgbToLinear(rgb255[0] / 255), g0 = srgbToLinear(rgb255[1] / 255), b0 = srgbToLinear(rgb255[2] / 255)
  const y = 0.2126 * r0 + 0.7152 * g0 + 0.0722 * b0
  const gain = y <= 0 ? 1 : toneTarget(y, exposure, rolloff) / y
  const pr = r0 * gain, pg = g0 * gain, pb = b0 * gain
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
  await ensureBackend()
  const exposure = 574 / 203
  const rolloff = 0.45
  const settings = { hdrIntensity: 1.5, rolloff, outputFormat: 'jpg_icc', outputGamut: '2020', whiteNits: 203, peakNits: 574, quality: 0.95 }
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
    const exp = expected(rgb, exposure, rolloff)
    const d = Math.max(Math.abs(actual[0] - exp[0]), Math.abs(actual[1] - exp[1]), Math.abs(actual[2] - exp[2]))
    // 旧 sRGB 值（纯蓝 [0,0,255]、纯红 [255,0,0]）与新 PQ 码差异应很大
    const ok = d <= 14
    if (!ok) allOk = false
    console.log(`${ok ? '✅' : '❌'} ${name}: 实际=${actual.join(',')} 预期Rec2020PQ=${exp.map(v => Math.round(v)).join(',')} 差=${d.toFixed(1)}（旧sRGB=${rgb.join(',')}）`)
  }
  stopBackend()
  console.log(allOk ? '\n✅ 图片 jpg_icc 已为 Rec.2020/PQ（B 曲线）' : '\n❌ 不匹配')
  process.exit(allOk ? 0 : 1)
}
main().catch((e) => { console.error('❌', e.message); stopBackend(); process.exit(1) })
