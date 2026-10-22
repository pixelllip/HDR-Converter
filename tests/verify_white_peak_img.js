/**
 * 验证图片 Ultra HDR 增益图的白点滚降（B 曲线，2026-10）
 *
 * 旧版测的是 hdrIntensity 封顶（A 曲线）；B 曲线不再使用 hdrIntensity，
 * 改为验证共用参数 rolloff：rolloff=1 的 maxBoost 应高于 rolloff=0，
 * 且任何情况下 maxBoost 不超过曝光（峰值/白点）。
 */
const fs = require('fs')
const path = require('path')
const { ensureBackend, stopBackend, httpJson } = require('./backend_test_util')
const { detectAndExtractGainMap } = require('./hdr-gainmap')

const SRC = 'D:\\video\\output\\img_238.jpg'  // 亮图（有高光）
const TMP = path.join(__dirname, 'tmp_video_hdr')

function maxBoostFromFile(jpegPath) {
  const r = detectAndExtractGainMap(fs.readFileSync(jpegPath), '.jpg')
  if (!r || !r.hasGainMap) return null
  return Math.pow(2, r.metadata.gainMapMaxLog2)
}

async function convertUhdr(peakNits, rolloff, out) {
  const r = await httpJson('POST', '/convert', {
    inputPath: SRC,
    outputPath: out,
    settings: { rolloff, outputFormat: 'jpg', whiteNits: 203, peakNits }
  })
  if (!r.success) throw new Error(r.message)
  return maxBoostFromFile(out)
}

async function main() {
  if (!fs.existsSync(SRC)) { console.log('缺少素材 img_238.jpg，跳过'); return }
  await ensureBackend()
  const peakNits = 1000
  const exposure = peakNits / 203
  const noRolloff = await convertUhdr(peakNits, 0.0, path.join(TMP, 'wp_rolloff0.jpg'))
  const fullRolloff = await convertUhdr(peakNits, 1.0, path.join(TMP, 'wp_rolloff1.jpg'))
  console.log(`rolloff=0 maxBoost = ${noRolloff ? noRolloff.toFixed(2) : null}`)
  console.log(`rolloff=1 maxBoost = ${fullRolloff ? fullRolloff.toFixed(2) : null}（应≤曝光 ${exposure.toFixed(2)}）`)
  const ok = noRolloff != null && fullRolloff != null && fullRolloff > noRolloff && fullRolloff <= exposure + 1e-6
  console.log(ok ? '✅ 白点滚降生效且不超过峰值' : '❌ 白点滚降异常')
  stopBackend()
  process.exit(ok ? 0 : 1)
}
main().catch((e) => { console.error('❌', e.message); stopBackend(); process.exit(1) })
