'use strict'
/**
 * _debug_probe.js — 定向诊断：本地 gyan essentials 9.0 到底在哪条路径能解析 2094-50
 *  1) HEVC Prefix_SEI（已知：期望 miss）
 *  2) HEVC Suffix_SEI
 *  3) AV1 元数据 OBU（METADATA / ITU-T T.35，obu_type=5）
 * 每条都跑 ffprobe -export_side_data 1 -show_frames 并汇报是否出现 App5 字段。
 */
const fs = require('fs')
const path = require('path')
const { spawnSync } = require('child_process')
const s50 = require('./st2094_50')
const inject = require('./hevc_inject')

const WORK = path.join(__dirname, '.work')
fs.mkdirSync(WORK, { recursive: true })
const FFMPEG = path.resolve(__dirname, '..', '..', '..', 'backend', 'ffmpeg', 'ffmpeg.exe')
const FFPROBE = path.resolve(__dirname, '..', '..', '..', 'backend', 'ffmpeg', 'ffprobe.exe')

function sh(bin, args) {
  const r = spawnSync(bin, args, { stdio: ['ignore', 'ignore', 'pipe'] })
  if (r.status !== 0) throw new Error(bin + ' exit=' + r.status + (r.stderr ? ' ' + r.stderr.toString().split('\n').filter(Boolean).slice(-3).join(' | ') : ''))
}

function probe(target, { label }) {
  const js = path.join(WORK, 'p.json')
  sh(FFPROBE, ['-v', 'error', '-export_side_data', '1', '-of', 'json', '-show_frames', '-o', js, target])
  const j = JSON.parse(fs.readFileSync(js, 'utf8'))
  let foundField = null
  for (const f of (j.frames || [])) {
    if ('application_version' in f || 'has_custom_hdr_reference_white_flag' in f || 'has_adaptive_tone_map_flag' in f) {
      foundField = { appVer: f.application_version, custom: f.has_custom_hdr_reference_white_flag, atm: f.has_adaptive_tone_map_flag, base: f.baseline_hdr_headroom, rw: f.use_reference_white_tone_mapping_flag, refWhite: f.hdr_reference_white }
      break
    }
  }
  console.log((foundField ? '  [HIT ] ' : '  [miss] ') + label + (foundField ? '  → ' + JSON.stringify(foundField) : ''))
  return !!foundField
}

// ---------- AV1 元数据 OBU 构造 ----------
function leb128(v) {
  const out = []
  do { let b = v & 0x7F; v >>>= 7; if (v > 0) b |= 0x80; out.push(b) } while (v > 0)
  return out
}
/** 构造一个 metadata(OBU_METADATA type=5, has_size=1) OBU，内含 ITU-T T.35 载荷 */
function av1MetadataObu(t35Buf) {
  const payload = Buffer.concat([Buffer.from([0x01]), t35Buf]) // metadata_type=1 (ITU-T T.35)
  const sizeField = Buffer.from(leb128(payload.length))
  const header = Buffer.from([0b00101010]) // f=0, type=5<<3, ext=0, size=1, res=0 → 0x2A
  return Buffer.concat([header, sizeField, payload])
}
/** 把 OBU 注入 IVF 首帧（紧随 32B 文件头 + 首帧 16B 帧头之后的 OBU 序列前） */
function injectAv1ObuAtFirstFrame(ivfBuf, obu) {
  if (ivfBuf.length < 48) throw new Error('IVF 过小')
  const frameSize = ivfBuf.readUInt32LE(32)
  const firstFrameEnd = 48 + frameSize
  if (firstFrameEnd > ivfBuf.length) throw new Error('首帧越界')
  return Buffer.concat([
    ivfBuf.subarray(0, 48),   // 文件头 + 首帧帧头
    Buffer.from([0, 0, 0, 1]), obu,
    ivfBuf.subarray(48)
  ])
}

// ---------- 准备数据 ----------
const payloadV1 = s50.t35Payload(s50.vectorMinimalDefault())
const payloadV2 = s50.t35Payload(s50.vectorReferenceWhiteRecipe(20000))
const src = path.join(WORK, 'src.h265')
if (!fs.existsSync(src)) {
  sh(FFMPEG, ['-hide_banner', '-y', '-f', 'lavfi', '-i', 'color=black:s=64x64:r=25:d=1.2',
    '-c:v', 'libx265', '-preset', 'ultrafast', '-crf', '30', '-pix_fmt', 'yuv420p10le',
    '-x265-params', 'aud=1:repeat-headers=1', '-f', 'hevc', src])
}

console.log('-- HEVC Paths --')
const pfx = path.join(WORK, 'dbg_prefix.h265')
inject.injectSei(src, pfx, payloadV2)
probe(pfx, { label: 'HEVC Prefix_SEI (V2 recipe H=2)' })

const sfx = path.join(WORK, 'dbg_suffix.h265')
const outSfx = inject.injectSeiPerAu(fs.readFileSync(src), () => payloadV2,
  { buildSeiNal: s50.buildSuffixSeiNal, position: 'before-aud' })
fs.writeFileSync(sfx, outSfx)
probe(sfx, { label: 'HEVC Suffix_SEI (V2 recipe H=2)' })

console.log('-- AV1 Path --')
const av1 = path.join(WORK, 'dbg.ivf')
if (!fs.existsSync(av1)) {
  sh(FFMPEG, ['-hide_banner', '-y', '-f', 'lavfi', '-i', 'color=black:s=64x64:r=25:d=0.6',
    '-c:v', 'libaom-av1', '-crf', '42', '-b:v', '0', '-g', '10', '-f', 'ivf', av1])
}
const av1Buf = fs.readFileSync(av1)
console.log('   ivf size=', av1Buf.length, 'header=', av1Buf.subarray(0, 36).toString('hex'))
const obu = av1MetadataObu(payloadV1)
const injected = injectAv1ObuAtFirstFrame(av1Buf, obu)
const av1inj = path.join(WORK, 'dbg_inj.ivf')
fs.writeFileSync(av1inj, injected)
probe(av1inj, { label: 'AV1 IVF + metadata OBU (V1 minimal)' })

console.log('\n-- 对照：无注入的 HEVC（应全 miss） --')
probe(src, { label: 'HEVC 原始（无注入）' })
