'use strict'
/**
 * scan_st2094_50.js — ST 2094-50 (Application #5) 检测器
 * 扫 HEVC/MP4 码流里的 `user_data_registered_itu_t_t35`（country B5 / provider 0090 / oriented 0001），
 * 解 Annex C app info，报告：总条数、各 distinct 载荷的 Hbaseline/参数、逐窗范围。
 *
 * 注：MediaInfo 等工具看不到它，是因为它们还没有 Application #5 解析；本脚本用 FFmpeg
 * 位流+自有解码器独立证明载荷存在且规范可解。
 *
 * 用法: node scan_st2094_50.js <file.mp4|.hevc|.h265>
 */
const { spawnSync } = require('child_process')
const fs = require('fs')
const path = require('path')
const s50 = require('./st2094_50')

const ROOT = path.resolve(__dirname, '..', '..', '..')
const FFMPEG = path.join(ROOT, 'backend', 'ffmpeg', 'ffmpeg.exe')
const { splitNalUnits, nalType } = require('./hevc_inject')
const WORK = path.join(__dirname, '.work')

function toAnnexB(file) {
  const ext = path.extname(file).toLowerCase()
  if (ext === '.hevc' || ext === '.h265' || ext === '.265') return fs.readFileSync(file)
  const tmp = path.join(WORK, 'scan_annexb.h265')
  fs.mkdirSync(WORK, { recursive: true })
  const errf = path.join(WORK, 'scan_err.log')
  const r = spawnSync(FFMPEG, ['-hide_banner', '-y', '-i', file, '-c', 'copy', '-bsf:v', 'hevc_mp4toannexb', '-f', 'hevc', tmp],
    { stdio: ['ignore', 'ignore', fs.openSync(errf, 'w')] })
  if (r.status !== 0) throw new Error('annexb 转换失败 exit=' + r.status)
  return fs.readFileSync(tmp)
}
function deEbsp(buf) {
  const out = []
  let z = 0
  for (const b of buf) {
    if (b === 3 && z >= 2) { z = 0; continue }
    out.push(b); z = (b === 0) ? z + 1 : 0
  }
  return Buffer.from(out)
}
function findApp(nalRaw) {
  const d = deEbsp(nalRaw)
  const m = Buffer.from([0xB5, 0x00, 0x90, 0x00, 0x01])
  const i = d.indexOf(m)
  if (i < 0) return null
  try { return s50.decodeApplicationInfo(d.subarray(i + 5)) } catch (e) { return null }
}

function main() {
  const file = process.argv[2]
  if (!file || !fs.existsSync(file)) { console.error('用法: node scan_st2094_50.js <file>'); process.exit(2) }
  const buf = toAnnexB(file)
  const nals = splitNalUnits(buf)
  // 有 AUD 就把 SEI 归到帧号
  const audIdx = []
  const seen = []
  let cur = -1
  for (let i = 0; i < nals.length; i++) {
    const t = nalType(nals, i)
    if (t === 35) { cur++; continue }
    if (t === 39) {
      const app = findApp(nals[i].raw)
      if (app) seen.push({ au: cur, info: app.info })
    }
  }
  console.log('文件      :', file)
  console.log('码流大小  :', buf.length, 'bytes')
  console.log('AUD 数    :', audIdx.length || '(无 AUD，按出现顺序)')
  console.log('检出 ST 2094-50 (T.35/App5) SEI 总数:', seen.length)
  if (!seen.length) { console.log('\n→ 该文件不含 ST 2094-50 动态元数据（仅静态 HDR10/其它）'); return 1 }

  // 按 continuous runs 分组（baseline 相同的连续段 = 一个窗）
  const runs = []
  for (const e of seen) {
    const atm = e.info.colorVolumeTransform.adaptiveToneMap
    const base = atm ? atm.baselineHdrHeadroom : null
    if (runs.length && runs[runs.length - 1].base === base) {
      runs[runs.length - 1].end = e.au
      runs[runs.length - 1].cnt++
    } else runs.push({ base, start: e.au, end: e.au, cnt: 1, info: e.info })
  }
  console.log('\nHbaseline 分段（连续相同 = 同一窗）：')
  for (const r of runs) {
    const atm = r.info.colorVolumeTransform.adaptiveToneMap
    const cvt = r.info.colorVolumeTransform
    const line = `  ${r.cnt.toString().padStart(3)} 条  ${r.start}~${r.end}  baseline=${r.base} (${((r.base || 0) / 10000).toFixed(3)}档)`
      + `  has_custom_ref_white=${cvt.hasCustomHdrReferenceWhite}`
      + `  has_atm=${cvt.hasAdaptiveToneMap}  useRW=${atm ? atm.useReferenceWhiteToneMapping : '-'}`
    console.log(line)
  }
  console.log('\n→ 该文件**包含** ST 2094-50 (Application #5) 动态元数据（T.35: country 0xB5 / provider 0x0090 / oriented 0x0001 + Annex C）')
  return 0
}
process.exit(main())
