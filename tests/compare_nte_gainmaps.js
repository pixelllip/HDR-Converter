/**
 * 视觉/数值对比脚本：用 nte_bg_0.png (1920x1080)，对比三套增益图下采样方案。
 *
 * - step1-bilinear：全分辨率逐像素 ratio → 双线性 decimation（缺低通，aliasing）
 * - step1-box     ：全分辨率逐像素 ratio → box-average decimation（第一步改动后）
 * - step2         ：box 下采样主图 → 低分辨率 mask/gain（当前主链路）
 *
 * 输入（在 tests/ 下由 `cargo run --example dump_gainmap_compare` 生成）：
 *   - gainmap_step1_fullres_bilinear.png  (480x270)
 *   - gainmap_step1_fullres_box.png       (480x270)
 *   - gainmap_step2_lowres.png            (480x270)
 *   - gainmap_step1_bilinear_vs_step2_diff.png
 *   - gainmap_step1_box_vs_step2_diff.png
 *   - nte_bg_0_ultrahdr_step2.jpg         （完整 Ultra HDR，step2 主链路产物）
 *
 * 输出：
 *   - gainmap_compare_report.json
 */

const fs = require('fs')
const path = require('path')
const sharp = require('sharp')
const { detectAndExtractGainMap } = require('./hdr-gainmap')

const TESTS = path.resolve(__dirname)
const INPUT = path.join(TESTS, 'nte_bg_0_input.png')
const S1B = path.join(TESTS, 'gainmap_step1_fullres_bilinear.png')
const S1X = path.join(TESTS, 'gainmap_step1_fullres_box.png')
const S2 = path.join(TESTS, 'gainmap_step2_lowres.png')
const S3 = path.join(TESTS, 'gainmap_step3_lowres_maskblur.png')
const DIFF_B = path.join(TESTS, 'gainmap_step1_bilinear_vs_step2_diff.png')
const DIFF_X = path.join(TESTS, 'gainmap_step1_box_vs_step2_diff.png')
const DIFF_23 = path.join(TESTS, 'gainmap_step2_vs_step3_diff.png')
const UHDR = path.join(TESTS, 'nte_bg_0_ultrahdr_step3.jpg')

async function loadGray(p) {
  const { data, info } = await sharp(p).greyscale().raw().toBuffer({ resolveWithObject: true })
  return { data, w: info.width, h: info.height }
}

function rmse(a, b) {
  if (a.length !== b.length) throw new Error('size mismatch')
  let s = 0
  for (let i = 0; i < a.length; i++) s += (a[i] - b[i]) ** 2
  return Math.sqrt(s / a.length)
}

function diffStats(diff) {
  let max = 0
  let sum = 0n
  let big = 0
  for (let i = 0; i < diff.length; i++) {
    const v = diff[i]
    if (v > max) max = v
    sum += BigInt(v)
    if (v > 32) big++
  }
  return {
    max_abs: max,
    mean_abs: Number(sum) / diff.length,
    big_diff_count: big,
    big_diff_ratio: Number(((100 * big) / diff.length).toFixed(2)),
  }
}

function findInfoRow(img) {
  let bestY = -1, bestX = -1, bestScore = -1
  for (let y = 0; y < img.h; y++) {
    let mx = 0, mxX = -1
    for (let x = 0; x < img.w; x++) if (img.data[y * img.w + x] > mx) { mx = img.data[y * img.w + x]; mxX = x }
    let mn = 255
    for (let x = 0; x < img.w; x++) if (img.data[y * img.w + x] < mn) mn = img.data[y * img.w + x]
    const zeros = Array.from(img.data.slice(y * img.w, (y + 1) * img.w)).filter((v) => v < 4).length
    const score = (mx - mn) * (0.3 + zeros / img.w)
    if (score > bestScore) { bestScore = score; bestY = y; bestX = mxX }
  }
  return { y: bestY, x: bestX }
}

function monotonicSegments(arr) {
  if (arr.length <= 1) return 1
  let segs = 1
  for (let i = 1; i < arr.length; i++) if (arr[i] < arr[i - 1]) segs++
  return segs
}

function maxJump(arr) {
  let m = 0
  for (let i = 1; i < arr.length; i++) {
    const d = Math.abs(arr[i] - arr[i - 1])
    if (d > m) m = d
  }
  return m
}

async function main() {
  console.log('加载测试产物...')
  for (const p of [INPUT, S1B, S1X, S2, S3, DIFF_B, DIFF_X, DIFF_23, UHDR]) {
    if (!fs.existsSync(p)) {
      console.error('缺少文件:', p)
      process.exit(1)
    }
  }

  const s1b = await loadGray(S1B)
  const s1x = await loadGray(S1X)
  const s2 = await loadGray(S2)
  const s3 = await loadGray(S3)
  const mainImg = await loadGray(INPUT)
  console.log(`主图: ${mainImg.w}x${mainImg.h}`)
  console.log(`四套增益图均: ${s3.w}x${s3.h}`)

  // 四方对比
  function statsPair(a, b) {
    let max = 0, sum = 0n, big = 0
    for (let i = 0; i < a.length; i++) {
      const v = a[i] < b[i] ? b[i] - a[i] : a[i] - b[i]
      if (v > max) max = v
      sum += BigInt(v)
      if (v > 32) big++
    }
    return {
      max_abs: max,
      mean_abs: Number(sum) / a.length,
      big_diff_count: big,
      big_diff_ratio: Number(((100 * big) / a.length).toFixed(2)),
      rmse: rmse(a, b),
    }
  }
  const stat_s1b_vs_s3 = statsPair(s1b.data, s3.data)
  const stat_s1x_vs_s3 = statsPair(s1x.data, s3.data)
  const stat_s2_vs_s3 = statsPair(s2.data, s3.data)

  console.log('\n===== 全图 |差| 统计（vs step3，当前主链路） =====')
  console.log('step1-bilinear vs step3 :', stat_s1b_vs_s3)
  console.log('step1-box      vs step3 :', stat_s1x_vs_s3)
  console.log('step2          vs step3 :', stat_s2_vs_s3)

  // 找信息量最大行
  const info = findInfoRow(s3)
  console.log(`\n信息量最大行 y=${info.y}/${s3.h}, 峰 x=${info.x}`)
  const s1bRow = Array.from(s1b.data.slice(info.y * s3.w, (info.y + 1) * s3.w))
  const s1xRow = Array.from(s1x.data.slice(info.y * s3.w, (info.y + 1) * s3.w))
  const s2Row = Array.from(s2.data.slice(info.y * s3.w, (info.y + 1) * s3.w))
  const s3Row = Array.from(s3.data.slice(info.y * s3.w, (info.y + 1) * s3.w))
  const start = Math.max(0, Math.min(s3.w - 60, info.x - 30))
  console.log(`窗口 x=[${start}..${start + 60}]（60 列剖面）：`)
  console.log('  step1-bilinear:', s1bRow.slice(start, start + 60).join(','))
  console.log('  step1-box     :', s1xRow.slice(start, start + 60).join(','))
  console.log('  step2         :', s2Row.slice(start, start + 60).join(','))
  console.log('  step3         :', s3Row.slice(start, start + 60).join(','))
  console.log('  最大相邻跳变：')
  console.log(`    step1-bilinear: ${maxJump(s1bRow)}, 单调段数=${monotonicSegments(s1bRow)}`)
  console.log(`    step1-box     : ${maxJump(s1xRow)}, 单调段数=${monotonicSegments(s1xRow)}`)
  console.log(`    step2         : ${maxJump(s2Row)}, 单调段数=${monotonicSegments(s2Row)}`)
  console.log(`    step3         : ${maxJump(s3Row)}, 单调段数=${monotonicSegments(s3Row)}`)

  // 抽出 Ultra HDR 的内嵌增益图，与四套对比
  const uhdrBuf = fs.readFileSync(UHDR)
  const ext = detectAndExtractGainMap(uhdrBuf, '.jpg')
  let embed = null
  if (ext && ext.gainMapBase64) {
    const buf = Buffer.from(ext.gainMapBase64, 'base64')
    const { data, info: m } = await sharp(buf).greyscale().raw().toBuffer({ resolveWithObject: true })
    embed = { data, w: m.width, h: m.height, size: `${m.width}x${m.height}` }
    console.log(`\n===== Ultra HDR (step3) 抽出增益图 =====`)
    console.log(`尺寸=${embed.size}`)
    if (embed.w === s3.w && embed.h === s3.h) {
      console.log(`vs step1-bilinear RMSE = ${rmse(embed.data, s1b.data).toFixed(3)}`)
      console.log(`vs step1-box      RMSE = ${rmse(embed.data, s1x.data).toFixed(3)}`)
      console.log(`vs step2          RMSE = ${rmse(embed.data, s2.data).toFixed(3)}`)
      console.log(`vs step3          RMSE = ${rmse(embed.data, s3.data).toFixed(3)}`)
    }
  }

  // 报告
  const report = {
    input: path.basename(INPUT),
    step1_bilinear_gainmap: path.basename(S1B),
    step1_box_gainmap: path.basename(S1X),
    step2_gainmap: path.basename(S2),
    step3_gainmap: path.basename(S3),
    diff_2_vs_3: path.basename(DIFF_23),
    ultra_hdr_step3: path.basename(UHDR),
    info: { best_y: info.y, peak_x: info.x },
    bilinear_vs_step3: stat_s1b_vs_s3,
    box_vs_step3: stat_s1x_vs_s3,
    step2_vs_step3: stat_s2_vs_s3,
    embedded_in_uhdr: embed
      ? {
          size: embed.size,
          rmse_vs_step1_bilinear: rmse(embed.data, s1b.data),
          rmse_vs_step1_box: rmse(embed.data, s1x.data),
          rmse_vs_step2: rmse(embed.data, s2.data),
          rmse_vs_step3: rmse(embed.data, s3.data),
        }
      : null,
  }
  fs.writeFileSync(path.join(TESTS, 'gainmap_compare_report.json'), JSON.stringify(report, null, 2))
  console.log(`\n报告: ${path.join(TESTS, 'gainmap_compare_report.json')}`)
}

main().catch((e) => {
  console.error(e)
  process.exit(1)
})
