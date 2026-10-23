/**
 * 研究用：同一 SDR 素材走真实视频链路，只改「色域」下拉（= 输入解读 + 输出编码），
 * 观察输出 HDR10 的色度是否真的被拓展。
 *
 * 三种组合：
 *   A. input=709  output=2020  → 正确的 SDR→HDR 转封装（colorimetric）
 *   B. input=2020 output=2020  → 把 709 素材当 2020 解读（assign 式，会真的更艳）
 *   C. input=p3   output=p3    → 把 709 素材当 P3 解读 + 输出 P3
 */
const path = require('path')
const vc = require('../../video_converter')

const PORT = 45999
const DIR = __dirname
const SRC = path.join(DIR, 'patches_sdr.mp4')

const cases = [
  { tag: 'in709_out2020', inputPrimaries: '709', outputPrimaries: '2020' },
  { tag: 'in2020_out2020', inputPrimaries: '2020', outputPrimaries: '2020' },
  { tag: 'inp3_outp3', inputPrimaries: 'p3', outputPrimaries: 'p3' },
]

async function main() {
  for (const c of cases) {
    const out = path.join(DIR, `video_${c.tag}.mp4`)
    const settings = {
      peakNits: 574,
      whiteNits: 203,
      gamma: 1.0,
      crf: 12,
      encoder: 'x265',
      inputTransfer: 'srgb',
      inputPrimaries: c.inputPrimaries,
      outputTransfer: 'auto',
      outputPrimaries: c.outputPrimaries,
      rgbAdjustment: { red: 1, green: 1, blue: 1 },
    }
    await vc.convertVideoDirect(SRC, out, settings, { backendPort: PORT }, () => {})
    console.log('done', c.tag)
  }
}

main().catch((e) => {
  console.error('FAIL', e && e.stack || e)
  process.exit(1)
})
