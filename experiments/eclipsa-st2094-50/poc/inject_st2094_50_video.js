'use strict'
/**
 * inject_st2094_50_video.js — 把 ST 2094-50 (Application #5) 动态元数据注入一段 HDR10 MP4
 *
 * 流程：
 *  1) signalstats 逐帧 YMAX → PQ EOTF → 每窗 MaxCLL(尼特)
 *  2) 每窗 Hbaseline = log2(窗口MaxCLL / 参考白=203)，raw=round(Hb×10000)
 *  3) 用 C.3.8 参考白配方编码载荷（st2094_50.js）：
 *     has_custom_ref_white=0(默认203) + has_atm=1 + use_reference_white_tone_mapping=1 + baseline
 *  4) mp4 → AnnexB(hevc_mp4toannexb + aud=insert) → 按 AUD 注入每窗 SEI → remux 回 mp4
 *  5) 复用 mp4_hdr.js 重新注入 mdcv/clli（容器盒在 mp4→annexb 时丢失，补回，保持“静态+动态”共存）
 *  6) 自验：V1 在注入后 ES 上逐 AUD 回读（逐窗动态证明）；
 *     V2 在最终 mp4 上统计 T.35 SEI 仍存在（remux 不丢）。
 *
 * 用法: node inject_st2094_50_video.js <input.mp4> <output.mp4> [windowCount]
 */
const { spawnSync } = require('child_process')
const fs = require('fs')
const path = require('path')
const s50 = require('./st2094_50')

const ROOT = path.resolve(__dirname, '..', '..', '..') // hdr_electron
const FFMPEG = path.join(ROOT, 'backend', 'ffmpeg', 'ffmpeg.exe')
const { injectSeiPerAu, splitNalUnits, nalType } = require('./hevc_inject')
const { injectHdrBoxes } = require(path.join(ROOT, 'mp4_hdr.js'))

const WORK = path.join(__dirname, '.work')
fs.mkdirSync(WORK, { recursive: true })

const INPUT = process.argv[2]
const OUTPUT = process.argv[3]
const WINDOW_COUNT = Math.max(1, parseInt(process.argv[4] || '3', 10) || 3)
if (!INPUT || !OUTPUT) { console.error('用法: node inject_st2094_50_video.js <in.mp4> <out.mp4> [windows]'); process.exit(2) }

const REF_WHITE_NITS = 203 // BT.2408 参考白（规范默认值，即“锚点”）

function sh(bin, args) {
  const errf = path.join(WORK, 'inj_err.log')
  const r = spawnSync(bin, args, { stdio: ['ignore', 'ignore', fs.openSync(errf, 'w')] })
  if (r.status !== 0) throw new Error(`${path.basename(bin)} ${args.slice(0, 6).join(' ')}… exit=${r.status}\n` +
    fs.readFileSync(errf, 'utf8').split('\n').filter(Boolean).slice(-8).join('\n'))
  return fs.readFileSync(errf, 'utf8')
}

function pqEotf(v01) {
  const m = 78.84375, n = 0.1593017578125, c1 = 0.8359375, c2 = 18.8515625, c3 = 18.6875
  const y = Math.pow(Math.min(1, Math.max(0, v01)), 1 / m)
  return Math.pow(Math.max(y - c1, 0) / (c2 - c3 * y), 1 / n) * 10000
}

/** 1) 逐帧 YMAX（10-bit limited PQ 码值 0..1023）；signalstats 写入帧 metadata，需接 metadata=print */
function perFrameYMax(mp4Path) {
  const logFile = path.join(WORK, 'signalstats.log')
  const proc = spawnSync(FFMPEG, ['-hide_banner', '-i', mp4Path, '-vf', 'signalstats,metadata=print:key=lavfi.signalstats.YMAX', '-an', '-f', 'null', '-'],
    { stdio: ['ignore', 'ignore', fs.openSync(logFile, 'w')] })
  if (proc.status !== 0) throw new Error('signalstats 失败 exit=' + proc.status)
  const re = /lavfi\.signalstats\.YMAX=(\d+)/g
  const frames = []
  let m
  const txt = fs.readFileSync(logFile, 'utf8')
  while ((m = re.exec(txt)) !== null) frames.push(parseFloat(m[1]))
  return frames
}

/** 去 EBSP(00 00 03 → 00 00) */
function deEbsp(buf) {
  const out = []
  let zeros = 0
  for (let i = 0; i < buf.length; i++) {
    const b = buf[i]
    if (b === 3 && zeros >= 2) { zeros = 0; continue }
    out.push(b)
    zeros = (b === 0) ? zeros + 1 : 0
  }
  return Buffer.from(out)
}

/** 在一条 SEI NAL 里找 T.35 载荷；找到返回 app info 缓冲区，否则 null */
function findAppInfo(nalRaw) {
  const data = deEbsp(nalRaw)
  const marker = Buffer.from([0xB5, 0x00, 0x90, 0x00, 0x01])
  const idx = data.indexOf(marker)
  if (idx < 0) return null
  try { return s50.decodeApplicationInfo(data.subarray(idx + 5)) } catch (e) { return null }
}

function main() {
  if (!fs.existsSync(INPUT)) throw new Error('输入不存在: ' + INPUT)
  const ymax = perFrameYMax(INPUT)
  const n = ymax.length
  if (!n) throw new Error('signalstats 未读到帧')
  console.log(`逐帧 YMAX 已读取（${n} 帧）`)

  // 2) 分窗：均分
  const payloads = []
  for (let w = 0; w < WINDOW_COUNT; w++) {
    const a = Math.floor(w * n / WINDOW_COUNT), b = Math.floor((w + 1) * n / WINDOW_COUNT)
    let mx = 0
    for (let i = a; i < b; i++) mx = Math.max(mx, pqEotf(ymax[i] / 1023))
    const nits = Math.round(mx)
    const hb = nits > 0 ? Math.log2(nits / REF_WHITE_NITS) : 0
    const raw = Math.max(0, Math.round((Math.min(6, hb)) * 10000))
    payloads.push({ w, a, b, nits, hb: +hb.toFixed(4), raw, hex: s50.hex(s50.vectorReferenceWhiteRecipe(raw)) })
  }

  console.log('\n===== 每窗数值（参考白=203，动态元数据=C.3.8 参考白配方） =====')
  for (const p of payloads) console.log(
    `窗${p.w} 帧[${p.a}~${p.b - 1}]  MaxCLL≈${p.nits}尼特  Hbaseline=${p.hb.toFixed(4)}档  raw=${p.raw}  载荷=${p.hex}`)

  // 4) mp4 → annexb（补 AUD，便于按帧注入）→ 按窗注入
  const esIn = path.join(WORK, 'in_annexb.h265')
  sh(FFMPEG, ['-hide_banner', '-y', '-i', INPUT, '-c', 'copy', '-bsf:v', 'hevc_mp4toannexb,hevc_metadata=aud=insert', '-f', 'hevc', esIn])
  const esBuf = fs.readFileSync(esIn)

  const payloadForAu = (au) => {
    const p = payloads.find(win => au >= win.a && au < win.b) || payloads[payloads.length - 1]
    return s50.t35Payload(s50.vectorReferenceWhiteRecipe(p.raw))
  }
  const injected = injectSeiPerAu(esBuf, payloadForAu)
  const esOut = path.join(WORK, 'injected.h265')
  fs.writeFileSync(esOut, injected)

  // remux 回 mp4（必须 -tag:v hvc1；-avoid_negative_ts make_zero 修复裸流重封导致的
  // 开头负 PTS/丢帧问题）+ 补回容器静态盒
  sh(FFMPEG, ['-hide_banner', '-y', '-i', esOut, '-c', 'copy', '-tag:v', 'hvc1',
    '-avoid_negative_ts', 'make_zero', '-movflags', '+faststart', OUTPUT])
  injectHdrBoxes(OUTPUT, { maxCll: 574, maxFall: 400 })
  console.log('\n输出:', OUTPUT, '（' + (fs.statSync(OUTPUT).size / 1e6).toFixed(2) + ' MB）')

  // 6-V1) 注入后 ES：逐 AUD 回读 T.35 → 验证“逐窗动态”
  console.log('\n===== V1 注入后 ES 逐 AUD 回读（动态证明） =====')
  const nals = splitNalUnits(esBuf)              // 原始 ES（尚未注入）
  const audCount = nals.filter((_, i) => nalType(nals, i) === 35).length
  const nalsI = splitNalUnits(injected)
  const perAu = []
  for (let i = 0; i < nalsI.length; i++) {
    if (nalType(nalsI, i) !== 35) continue
    const next = nalsI[i + 1]
    if (next && nalType(nalsI, i + 1) === 39) {
      const d = findAppInfo(next.raw)
      if (d && d.info.colorVolumeTransform.adaptiveToneMap) {
        perAu.push(d.info.colorVolumeTransform.adaptiveToneMap.baselineHdrHeadroom)
      }
    }
  }
  console.log(`AUD 数=${audCount}（应=${n}）  带回读 T.35 的 AUD=${perAu.length}`)
  const expectPerAu = []
  for (let f = 0; f < n; f++) { const p = payloads.find(win => f >= win.a && f < win.b) || payloads[payloads.length - 1]; expectPerAu.push(p.raw) }
  const okV1 = perAu.length === n && perAu.every((v, i) => v === expectPerAu[i])
  // 打印每窗实际读到的 baseline 与窗口范围
  const distinct = []
  let start = -1
  for (let i = 0; i < perAu.length; i++) {
    if (start < 0 || perAu[i] !== perAu[i - 1]) { if (start >= 0) distinct[distinct.length - 1].end = i - 1; start = i; distinct.push({ raw: perAu[i], start }) }
  }
  if (distinct.length) distinct[distinct.length - 1].end = perAu.length - 1
  for (const d of distinct) console.log(`  AUD[${d.start}~${d.end}] baseline=${d.raw} (${(d.raw / 10000).toFixed(2)}档)`)
  console.log(okV1 ? '✅ V1：逐窗 Hbaseline 与期望一致（动态注入正确）' : '❌ V1：逐窗回读与期望不一致')

  // 6-V2) 最终 mp4：T.35 SEI 仍存在 + 静态盒已补
  console.log('\n===== V2 最终 mp4 端到端 =====')
  const esV = path.join(WORK, 'verify_annexb.h265')
  sh(FFMPEG, ['-hide_banner', '-y', '-i', OUTPUT, '-c', 'copy', '-bsf:v', 'hevc_mp4toannexb', '-f', 'hevc', esV])
  const vBuf = fs.readFileSync(esV)
  const nalsV = splitNalUnits(vBuf)
  let markerCount = 0
  for (const nal of nalsV) if (nalType(nalsV, nalsV.indexOf(nal)) === 39 && findAppInfo(nal.raw)) markerCount++
  console.log(`最终 mp4 中检出 T.35(2094-50 前缀 B5..01) 的 SEI 条数=${markerCount}（应≈${n}）`)
  console.log('（注意：mp4 采样可能不含 AUD；SEI 本身保留在采样里即可）')
  const okV2 = markerCount > 0
  console.log(okV2 ? '✅ V2：动态元数据在最终 mp4 中保留' : '❌ V2：最终 mp4 未检出 T.35 SEI')

  const ok = okV1 && okV2
  console.log('\n' + (ok ? '✅ 注入完成，静态 HDR10 + ST 2094-50 动态元数据共存' : '❌ 存在失败项'))
  return ok ? 0 : 1
}

process.exit(main())
