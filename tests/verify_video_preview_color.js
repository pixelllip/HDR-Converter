/**
 * 验证直接滤镜预览色彩与实际视频一致（2026-08-13）
 *
 * 旧 bug：预览 jpg_icc 用 sRGB 编码像素 + Rec.2020/PQ ICC → Chromium 按 Rec.2020/PQ 解释 sRGB 数值 → 色彩错乱。
 * 修复：/preview mode=videoDirect 走与视频输出一致的 Rec.709→Rec.2020→PQ 管线。
 *
 * 对比方式（都在线性 Rec.2020 域比，避免 zscale 范围/解码差异）：
 *   - 预览：解码 videoDirect JPEG → PQ 码 → 反 PQ → 线性(0..1 相对 10000 尼特) → 尼特
 *   - 视频：/video-frame transform → PAM → 用真实编码命令(libx265, npl=peak) → zscale 解码线性 → 尼特
 */
const fs = require('fs')
const path = require('path')
const { spawnSync } = require('child_process')
const { ensureBackend, stopBackend, httpJson } = require('./backend_test_util')

const FFMPEG = path.join(__dirname, '..', 'backend', 'ffmpeg', 'ffmpeg.exe')
const TMP = path.join(__dirname, 'tmp_video_hdr')
const FRAME_BLUE = path.join(TMP, 'color_blue.png')
const FRAME_RED = path.join(TMP, 'color_red.png')

if (!fs.existsSync(FRAME_BLUE)) spawnSync(FFMPEG, ['-y', '-f', 'lavfi', '-i', 'color=c=0x0000ff:size=64x64:rate=1', '-frames:v', '1', FRAME_BLUE], { windowsHide: true })
if (!fs.existsSync(FRAME_RED)) spawnSync(FFMPEG, ['-y', '-f', 'lavfi', '-i', 'color=c=0xff0000:size=64x64:rate=1', '-frames:v', '1', FRAME_RED], { windowsHide: true })

// PQ 反解（码 0..1 → 线性 0..1 相对 10000 尼特）
function pqInv(code) {
  const m1 = 0.1593017578125, m2 = 78.84375, c1 = 0.8359375, c2 = 18.8515625, c3 = 18.6875
  const cm2 = Math.pow(code, 1 / m2)
  return Math.pow(Math.max(cm2 - c1, 0) / (c2 - c3 * cm2), 1 / m1)
}

/** 预览 JPEG → 线性 Rec.2020（尼特），取 (x,y) 像素 */
function previewLinear(jpegBuf, w, h, x, y) {
  const tmp = path.join(TMP, 'pv_preview.jpg')
  fs.writeFileSync(tmp, jpegBuf)
  const out = spawnSync(FFMPEG, ['-v', 'error', '-i', tmp, '-f', 'rawvideo', '-pix_fmt', 'rgb24', '-'], { maxBuffer: 1 << 28, windowsHide: true })
  const idx = (y * w + x) * 3
  const r = out.stdout[idx] / 255, g = out.stdout[idx + 1] / 255, b = out.stdout[idx + 2] / 255
  return [pqInv(r) * 10000, pqInv(g) * 10000, pqInv(b) * 10000]
}

/** 视频：PAM → 真实编码 → zscale 解码线性(npl=100) → 尼特 */
function videoLinear(pamB64, npl, x, y, w, h) {
  const seq = path.join(TMP, 'pv_seq')
  fs.mkdirSync(seq, { recursive: true })
  const pamBytes = Buffer.from(pamB64, 'base64')
  const pamPath = path.join(seq, 'pv_000000.pam')
  fs.writeFileSync(pamPath, pamBytes)
  fs.writeFileSync(path.join(seq, 'pv_000001.pam'), pamBytes)
  const mp4 = path.join(seq, 'pv.mp4')
  const enc = spawnSync(FFMPEG, ['-y', '-framerate', '1', '-start_number', '0', '-i', pamPath,
    '-vf', `zscale=in_range=full:pin=bt709:tin=linear:npl=${npl}:p=bt2020:t=smpte2084:m=bt2020nc:r=limited,format=yuv420p10le`,
    '-c:v', 'libx265', '-crf', '18', '-tag:v', 'hvc1',
    '-color_primaries', 'bt2020', '-color_trc', 'smpte2084', '-colorspace', 'bt2020nc', '-color_range', 'tv',
    '-an', mp4], { encoding: 'utf8', windowsHide: true })
  if (enc.status !== 0) throw new Error('视频编码失败: ' + (enc.stderr || '').slice(-200))
  const dec = spawnSync(FFMPEG, ['-v', 'error', '-i', mp4, '-frames:v', '1',
    '-vf', 'zscale=tin=smpte2084:t=linear:npl=100:rin=limited:r=full,format=gbrpf32le',
    '-f', 'rawvideo', '-pix_fmt', 'gbrpf32le', '-'], { maxBuffer: 1 << 28, windowsHide: true })
  const n = w * h
  const idx = y * w + x
  const g = dec.stdout.readFloatLE(idx * 4)
  const b = dec.stdout.readFloatLE(n * 4 + idx * 4)
  const r = dec.stdout.readFloatLE(2 * n * 4 + idx * 4)
  return [r * 100, g * 100, b * 100]
}

async function main() {
  const port = await ensureBackend()
  const whiteNits = 203, peakNits = 574, npl = peakNits
  const settings = { gamma: 0.9, rgbAdjustment: { red: 1, green: 1, blue: 1 }, whiteNits, peakNits, quality: 0.95 }

  // 预览是 32×32（缩放50%），视频是 64×64（全帧）；纯色帧任意像素相同，统一取 (16,16)
  const cases = [
    ['纯蓝 (0,0,255)', FRAME_BLUE, 16, 16],
    ['纯红 (255,0,0)', FRAME_RED, 16, 16]
  ]
  let allOk = true
  for (const [name, frame, x, y] of cases) {
    const prev = await httpJson('POST', '/preview', { inputPath: frame, settings, mode: 'videoDirect' })
    const jpeg = Buffer.from(prev.dataUrl.split(',')[1], 'base64')
    const w = prev.width, h = prev.height

    const vf = await httpJson('POST', '/video-frame', {
      inputPath: frame, settings: { ...settings, hdrIntensity: peakNits / whiteNits }, peak: peakNits / whiteNits, mode: 'transform'
    })

    const pre = previewLinear(jpeg, w, h, x, y)
    // 视频是全帧尺寸（64×64），不能用预览的 32×32，否则 gbrpf32le 平面偏移错位
    const vid = videoLinear(vf.pamBase64, npl, x, y, vf.width, vf.height)
    const d = Math.max(Math.abs(pre[0] - vid[0]), Math.abs(pre[1] - vid[1]), Math.abs(pre[2] - vid[2]))
    const ok = d <= 25 // 尼特，容忍 8-bit/10-bit 量化 + JPEG
    if (!ok) allOk = false
    console.log(`${ok ? '✅' : '❌'} ${name}: 预览线性=[${pre.map(v => v.toFixed(1)).join(',')}] 视频线性=[${vid.map(v => v.toFixed(1)).join(',')}] 尼特 最大差=${d.toFixed(1)}`)
  }
  stopBackend()
  console.log(allOk ? '\n✅ 预览色彩与视频一致' : '\n❌ 仍不一致')
  process.exit(allOk ? 0 : 1)
}
main().catch((e) => { console.error('❌', e.message); stopBackend(); process.exit(1) })
