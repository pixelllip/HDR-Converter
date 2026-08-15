/**
 * 验证视频输出分辨率（2026-08-13）
 *
 * bug：2160p 素材转出来只有 1080p。根因：前端「处理宽度上限」默认 1920，
 *      且直接转模式隐藏该控件却仍生效 → scale='min(1920,iw)' 把 4K 压到 1920 宽。
 * 修复：默认 0=原始分辨率；上限提到 3840；两种模式都显示控件。
 *
 * 断言：2560×1440 素材，maxWidth=0 → 输出 2560×1440；maxWidth=1920 → 输出 1920×1080。
 */
const fs = require('fs')
const path = require('path')
const { spawn } = require('child_process')
const vc = require('../video_converter')
const { ensureBackend, stopBackend } = require('./backend_test_util')

const FFMPEG = path.join(__dirname, '..', 'backend', 'ffmpeg', 'ffmpeg.exe')
const FFPROBE = path.join(__dirname, '..', 'backend', 'ffmpeg', 'ffprobe.exe')
const SRC_DIR = 'D:\\video\\output'
const TMP = path.join(__dirname, 'tmp_video_hdr')

function run(cmd, args) {
  return new Promise((resolve, reject) => {
    const p = spawn(cmd, args, { windowsHide: true })
    let err = ''
    p.stderr.on('data', (d) => (err += d))
    p.on('error', reject)
    p.on('close', (code) => code === 0 ? resolve() : reject(new Error(`exit=${code}\n${err.slice(-400)}`)))
  })
}

async function probeW(p) {
  const out = await new Promise((resolve, reject) => {
    const c = spawn(FFPROBE, ['-v', 'error', '-select_streams', 'v:0', '-show_entries', 'stream=width,height', '-of', 'csv=p=0', p], { windowsHide: true })
    let o = ''
    c.stdout.on('data', (d) => (o += d))
    c.on('error', reject)
    c.on('close', (code) => code === 0 ? resolve(o.trim()) : reject(new Error('probe 失败')))
  })
  const m = out.match(/(\d+),(\d+)/)
  return m ? (m[1] + 'x' + m[2]) : out
}

async function main() {
  fs.mkdirSync(TMP, { recursive: true })
  const sdr4k = path.join(TMP, 'sdr_2560.mp4')
  // 用 2560×1440 素材合成测试视频（不缩放）
  await run(FFMPEG, ['-y', '-start_number', '0', '-framerate', '10', '-i', path.join(SRC_DIR, 'img_%d.jpg'),
    '-frames:v', '12', '-c:v', 'libx264', '-preset', 'veryfast', '-crf', '18', sdr4k])
  console.log('源视频:', sdr4k, '→', await probeW(sdr4k))

  const port = await ensureBackend()
  // maxWidth=0（新默认）→ 保持原始分辨率
  const out0 = path.join(TMP, 'res_maxw0.mp4')
  await vc.convertVideoDirect(sdr4k, out0, { hdrIntensity: 1.5, gamma: 0.9, crf: 22, maxWidth: 0, encoder: 'x265' },
    { backendPort: port }, () => {})
  const w0 = await probeW(out0)
  console.log('maxWidth=0 输出:', w0)
  console.log(w0 === '2560x1440' ? '✅ 保持原始分辨率' : '❌ 被缩小')

  // maxWidth=1920 → 压到 1920 宽（证明控件确实控制分辨率）
  const out1 = path.join(TMP, 'res_maxw1920.mp4')
  await vc.convertVideoDirect(sdr4k, out1, { hdrIntensity: 1.5, gamma: 0.9, crf: 22, maxWidth: 1920, encoder: 'x265' },
    { backendPort: port }, () => {})
  const w1 = await probeW(out1)
  console.log('maxWidth=1920 输出:', w1)
  console.log(w1 === '1920x1080' ? '✅ 按上限缩小' : '❌ 未按上限缩小')

  stopBackend()
  const ok = w0 === '2560x1440' && w1 === '1920x1080'
  console.log(ok ? '\n全部通过' : '\n存在失败')
  process.exit(ok ? 0 : 1)
}
main().catch((e) => { console.error('❌', e.message); stopBackend(); process.exit(1) })
