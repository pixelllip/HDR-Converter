/**
 * 验证图片 Ultra HDR 增益图峰值上限（2026-08-12）
 * hdrIntensity=3（maxBoost=8）但 peakNits/whiteNits 封顶时应被限制
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

async function convertUhdr(peakNits, out) {
  const r = await httpJson('POST', '/convert', {
    inputPath: SRC,
    outputPath: out,
    settings: { hdrIntensity: 3.0, fineTuneBrightness: 1.0, gamma: 0.9, outputFormat: 'jpg', whiteNits: 203, peakNits }
  })
  if (!r.success) throw new Error(r.message)
  return maxBoostFromFile(out)
}

async function main() {
  if (!fs.existsSync(SRC)) { console.log('缺少素材 img_238.jpg，跳过'); return }
  const port = await ensureBackend()
  // 无封顶（峰值很高）→ maxBoost 应为 2^3 = 8
  const noCap = await convertUhdr(100000, path.join(TMP, 'wp_nocap.jpg'))
  // 封顶（峰值 1000 / 白点 203 = 4.93）→ maxBoost 应 ≤ 4.93
  const capped = await convertUhdr(1000, path.join(TMP, 'wp_cap.jpg'))
  console.log(`hdrIntensity=3 无封顶 maxBoost = ${noCap ? noCap.toFixed(2) : null}（应≈8）`)
  console.log(`peakNits=1000/white=203 maxBoost = ${capped ? capped.toFixed(2) : null}（应≤4.93，被峰值封顶）`)
  const ok = noCap > 6 && capped < 6
  console.log(ok ? '✅ 峰值上限生效' : '❌ 峰值上限未生效')
  stopBackend()
  process.exit(ok ? 0 : 1)
}
main().catch((e) => { console.error('❌', e.message); stopBackend(); process.exit(1) })

