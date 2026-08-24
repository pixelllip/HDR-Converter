'use strict'
/**
 * dump_t35_bytes.js — 从视频里抠出 ST 2094-50 (App5) T.35 SEI 的原始字节并解码
 * 用法: node dump_t35_bytes.js <file.mp4>
 */
const { spawnSync } = require('child_process')
const fs = require('fs')
const path = require('path')
const s50 = require('./st2094_50')
const ROOT = path.resolve(__dirname, '..', '..', '..')
const FFMPEG = path.join(ROOT, 'backend', 'ffmpeg', 'ffmpeg.exe')
const { splitNalUnits, nalType } = require('./hevc_inject')
const WORK = path.join(__dirname, '.work')

function annexbOf(file) {
  const ext = path.extname(file).toLowerCase()
  if (ext === '.hevc' || ext === '.h265' || ext === '.265') return fs.readFileSync(file)
  fs.mkdirSync(WORK, { recursive: true })
  const tmp = path.join(WORK, 'dump_annexb.h265')
  const errf = path.join(WORK, 'dump_err.log')
  const r = spawnSync(FFMPEG, ['-hide_banner', '-y', '-i', file, '-c', 'copy', '-bsf:v', 'hevc_mp4toannexb', '-f', 'hevc', tmp],
    { stdio: ['ignore', 'ignore', fs.openSync(errf, 'w')] })
  if (r.status !== 0) throw new Error('annexb fail exit=' + r.status)
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

const file = process.argv[2]
if (!file || !fs.existsSync(file)) { console.error('usage: node dump_t35_bytes.js <file>'); process.exit(2) }
const buf = annexbOf(file)
const nals = splitNalUnits(buf)
let shown = 0
let frame = 0
const seen = new Set()
for (let i = 0; i < nals.length; i++) {
  const t = nalType(nals, i)
  if (t === 35) { frame++; continue }
  if (t !== 39) continue
  const d = deEbsp(nals[i].raw)
  const m = Buffer.from([0xB5, 0x00, 0x90, 0x00, 0x01])
  const idx = d.indexOf(m)
  if (idx < 0) continue
  const full = d.subarray(idx)                 // B5..01 + Annex C
  const app = full.subarray(5)
  const key = s50.hex(app)
  if (seen.has(key)) continue                  // 每个不同载荷只打一次
  seen.add(key)
  const dec = s50.decodeApplicationInfo(app)
  console.log(`帧#${frame}  T.35 载荷(${full.length}B): ${s50.hex(full)}`)
  console.log(`          ├→ Annex C 载荷: ${s50.hex(app)}`)
  const cvt = dec.info.colorVolumeTransform
  const atm = cvt.adaptiveToneMap || {}
  console.log(`          └→ 解码: has_custom_ref_white=${cvt.hasCustomHdrReferenceWhite} has_atm=${cvt.hasAdaptiveToneMap}` +
    ` useRW=${atm.useReferenceWhiteToneMapping ?? '-'} baseline=${atm.baselineHdrHeadroom ?? '-'}`)
  shown++
  if (shown >= 6) break
}
if (!shown) console.log('未找到 T.35/App5 载荷（说明文件不含 ST 2094-50）')
