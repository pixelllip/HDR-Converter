/**
 * 验证视频白点/峰值亮度参数（2026-08-12）
 * 转换 gainmap 模式视频，ffprobe 检查 max-cll 是否等于设定的峰值亮度
 */
const path = require('path')
const { spawnSync } = require('child_process')
const vc = require('../video_converter')
const { ensureBackend, stopBackend } = require('./backend_test_util')

const PROBE = path.join(__dirname, '..', 'backend', 'ffmpeg', 'ffprobe.exe')
const SDR = path.join(__dirname, 'tmp_video_hdr', 'sdr_test.mp4')

async function probeMaxCll(file) {
  const r = spawnSync(PROBE, ['-v', 'error', '-select_streams', 'v:0', '-show_frames',
    '-show_entries', 'frame=side_data_list', '-of', 'json', file], { encoding: 'utf8' })
  const j = JSON.parse(r.stdout)
  const sd = (j.frames && j.frames[0] && j.frames[0].side_data_list || [])
    .find((d) => d.side_data_type === 'Content light level metadata')
  return sd ? sd.max_content : null
}

async function main() {
  const port = await ensureBackend()
  for (const peakNits of [1000, 2000]) {
    const out = path.join(__dirname, 'tmp_video_hdr', `wb_peak${peakNits}.mp4`)
    await vc.convertVideoFrames(SDR, out,
      { hdrIntensity: 2.0, gamma: 0.9, crf: 22, maxWidth: 640, encoder: 'nvenc', whiteNits: 203, peakNits },
      { backendPort: port }, () => {})
    const cll = await probeMaxCll(out)
    console.log(`peakNits=${peakNits} → max-cll=${cll}  ${cll === peakNits ? '✅' : '❌'}`)
    if (cll !== peakNits) process.exitCode = 1
  }
  stopBackend()
  console.log('\n完成')
}
main().catch((e) => { console.error('❌', e.message); stopBackend(); process.exit(1) })
