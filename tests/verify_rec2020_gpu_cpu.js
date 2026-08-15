/**
 * 验证图片 Rec.2020/PQ 的 GPU 与 CPU 路径一致性 + 亮区不冲白（2026-08-13）
 * GPU/CPU 都无自动伽马、曝光=峰值/白点。
 */
const { spawnSync } = require('child_process')
const { ensureBackend, stopBackend, httpJson } = require('./backend_test_util')

const FFMPEG = 'c:/Users/Administrator/Documents/Java/hdr_electron/backend/ffmpeg/ffmpeg.exe'
const TMP = 'c:/Users/Administrator/Documents/Java/hdr_electron/tests/tmp_video_hdr'
const SRC = 'D:/video/output/img_0.jpg'

function stats(f) {
  const d = spawnSync(FFMPEG, ['-v', 'error', '-i', f, '-f', 'rawvideo', '-pix_fmt', 'rgb24', '-'], { maxBuffer: 1 << 28, windowsHide: true }).stdout
  const n = Math.floor(d.length / 3)
  let max = 0, sum = 0
  for (let i = 0; i < n; i++) { const v = (d[i * 3] + d[i * 3 + 1] + d[i * 3 + 2]) / 3; if (v > max) max = v; sum += v }
  return { max: Math.round(max), mean: (sum / n).toFixed(1) }
}

async function convert(disableGpu) {
  process.env.HDR_GPU_DISABLE = disableGpu ? '1' : ''
  const port = await ensureBackend()
  const out = TMP + '/gpc_' + (disableGpu ? 'cpu' : 'gpu') + '.jpg'
  const r = await httpJson('POST', '/convert', {
    inputPath: SRC, outputPath: out,
    settings: { hdrIntensity: 1.5, gamma: 0.9, outputFormat: 'jpg_icc', whiteNits: 203, peakNits: 574, quality: 0.95 }
  })
  stopBackend()
  if (!r.success) throw new Error('convert 失败: ' + (r.message || ''))
  return out
}

async function main() {
  const outG = await convert(false)
  const outC = await convert(true)
  delete process.env.HDR_GPU_DISABLE
  const g = stats(outG), c = stats(outC)
  console.log('GPU PQ: max=' + g.max + ' mean=' + g.mean)
  console.log('CPU PQ: max=' + c.max + ' mean=' + c.mean)
  const same = Math.abs(g.max - c.max) <= 2 && Math.abs(parseFloat(g.mean) - parseFloat(c.mean)) <= 2
  const notBlown = g.max < 235 // 不应冲白到 255（亮部保留细节）
  console.log('GPU/CPU 一致: ' + (same ? '✅' : '❌') + '  亮区不冲白(max<' + 235 + '): ' + (notBlown ? '✅' : '❌'))
  process.exit(same && notBlown ? 0 : 1)
}
main().catch((e) => { console.error('❌', e.message); process.exit(1) })
