'use strict'
/**
 * st2094_50_inject.js — 产品模块：给一段 HDR10（HEVC）MP4 附加 SMPTE ST 2094-50
 * (Application #5 / Eclipsa Video) 动态元数据，输出「HDR10 + 动态」共存文件。
 *
 * 流程：
 *   1) signalstats 逐帧 YMAX → PQ EOTF → 每窗 MaxCLL(尼特)
 *   2) 窗口划分：scene(镜头切检测) 或 uniform(均分)；每窗 Hbaseline = log2(MaxCLL/参考白)
 *   3) C.3.8 参考白配方编码载荷（st2094_50.js）
 *   4) mp4 → AnnexB(hevc_mp4toannexb + hevc_metadata=aud=insert) → 按 AUD 注入每窗 SEI
 *   5) remux 回 mp4(-tag:v hvc1 -avoid_negative_ts make_zero) → mp4_hdr.js 补回 mdcv/clli
 *
 * API:  attachSt2094_50(inputPath, outputPath, opts) → { outputPath, windows, totalSei }
 */
const fs = require('fs')
const path = require('path')
const os = require('os')
const { spawnSync } = require('child_process')
const s50 = require('./st2094_50')
const { splitNalUnits, nalType, injectSeiPerAu } = require('./hevc_inject')
const { injectHdrBoxes } = require('./mp4_hdr')

function tmpDir() { return fs.mkdtempSync(path.join(os.tmpdir(), 'hdr_eclipsa_')) }

/** 运行并返回 stderr 文本（stdout/stderr 重定向到文件再读，兼容沙箱/管道限制） */
function sh(bin, args, errFile) {
  const r = spawnSync(bin, args, { stdio: ['ignore', 'ignore', errFile ? fs.openSync(errFile, 'w') : 'pipe'] })
  if (r.status !== 0) {
    const err = (r.stderr ? r.stderr.toString() : (errFile && fs.existsSync(errFile) ? fs.readFileSync(errFile, 'utf8') : ''))
    throw new Error(`${path.basename(bin)} ${args.slice(0, 6).join(' ')}… exit=${r.status}\n${err.split('\n').filter(Boolean).slice(-8).join('\n')}`)
  }
  return errFile ? fs.readFileSync(errFile, 'utf8') : (r.stderr ? r.stderr.toString() : '')
}

function pqEotf(v01) {
  const m = 78.84375, n = 0.1593017578125, c1 = 0.8359375, c2 = 18.8515625, c3 = 18.6875
  const y = Math.pow(Math.min(1, Math.max(0, v01)), 1 / m)
  return Math.pow(Math.max(y - c1, 0) / (c2 - c3 * y), 1 / n) * 10000
}

/** 逐帧 YMAX（10-bit limited PQ 码值 0..1023） */
function perFrameYMax(mp4, ffmpeg, errFile) {
  const txt = sh(ffmpeg, ['-hide_banner', '-i', mp4, '-vf', 'signalstats,metadata=print:key=lavfi.signalstats.YMAX', '-an', '-f', 'null', '-'], errFile)
  const frames = []
  const re = /lavfi\.signalstats\.YMAX=(\d+)/g
  let m
  while ((m = re.exec(txt)) !== null) frames.push(parseFloat(m[1]))
  return frames
}

/** 镜头切检测：返回切割帧下标数组（升序） */
function sceneCuts(mp4, ffmpeg, fps, threshold, errFile) {
  const txt = sh(ffmpeg, ['-hide_banner', '-i', mp4, '-vf', `select='gt(scene,${threshold})',showinfo`, '-an', '-f', 'null', '-'], errFile)
  const times = []
  const re = /pts_time:([0-9.]+)/g
  let m
  while ((m = re.exec(txt)) !== null) times.push(parseFloat(m[1]))
  return times.map(t => Math.max(0, Math.round(t * fps)))
}

function buildWindows({ frameCount, fps, cuts, scheme, uniformWindows, minWindowSec }) {
  let bounds = []
  if (scheme === 'scene' && cuts && cuts.length) {
    // 合并过近的切点（< minWindowSec），并保证至少 2 段，否则回退 uniform
    const minFrames = Math.max(1, Math.round(minWindowSec * fps))
    for (const c of cuts) if (c >= minFrames && c <= frameCount - minFrames && (bounds.length === 0 || c - bounds[bounds.length - 1] >= minFrames)) bounds.push(c)
  }
  if (bounds.length < 1) {
    const n = Math.max(1, uniformWindows || 3)
    for (let w = 1; w < n; w++) bounds.push(Math.round(frameCount * w / n))
  }
  const wins = []
  let start = 0
  for (const b of [...bounds, frameCount]) {
    const end = Math.min(frameCount, b)
    if (end > start) { wins.push({ start, end }); start = end }
  }
  if (!wins.length) wins.push({ start: 0, end: frameCount })
  return wins
}

/**
 * 附加 ST 2094-50 动态元数据
 * @param inputPath   HDR10(HEVC) MP4（由本应用视频链路产出）
 * @param outputPath  输出（应 .mp4）
 * @param opts { ffmpeg, ffprobe, refWhiteNits=203, maxCll, maxFall,
 *              windowScheme='scene', uniformWindows=3, sceneThreshold=0.4, minWindowSec=0.5, onProgress }
 */
async function attachSt2094_50(inputPath, outputPath, opts = {}) {
  const ffmpeg = opts.ffmpeg, ffprobe = opts.ffprobe || opts.ffmpeg
  if (!ffmpeg || !fs.existsSync(inputPath)) throw new Error('attachSt2094_50: 参数缺失或输入不存在')
  const refWhite = Number(opts.refWhiteNits) || 203
  const maxCll = Number.isFinite(opts.maxCll) ? Math.round(opts.maxCll) : 574
  const maxFall = Number.isFinite(opts.maxFall) ? Math.round(opts.maxFall) : 400
  const scheme = opts.windowScheme === 'uniform' ? 'uniform' : 'scene'
  const onProgress = opts.onProgress || (() => {})
  const work = tmpDir()

  try {
    onProgress(0.5, 'Eclipsa：统计逐窗亮度…')
    const err1 = path.join(work, 'stats.log')
    const ymax = perFrameYMax(inputPath, ffmpeg, err1)
    const frameCount = ymax.length
    if (!frameCount) throw new Error('signalstats 未读到帧数')

    // 探测 fps
    let fps = 30
    const fpsOut = sh(ffprobe, ['-v', 'error', '-select_streams', 'v:0', '-show_entries', 'stream=avg_frame_rate', '-of', 'default=noprint_wrappers=1:nokey=1', inputPath], path.join(work, 'fps.log'))
    const mm = fpsOut.trim().match(/(\d+(?:\.\d+)?)(?:\/(\d+(?:\.\d+)?))?/)
    if (mm) fps = mm[2] ? (Number(mm[1]) / Number(mm[2])) : Number(mm[1])
    if (!(fps > 0)) fps = 30
    onProgress(0.6, 'Eclipsa：检测镜头/窗…')

    const cuts = scheme === 'scene' ? sceneCuts(inputPath, ffmpeg, fps, Number(opts.sceneThreshold) || 0.4, path.join(work, 'scene.log')) : []
    const windows = buildWindows({ frameCount, fps, cuts, scheme, uniformWindows: opts.uniformWindows || 3, minWindowSec: opts.minWindowSec || 0.5 })

    // 每窗 MaxCLL → Hbaseline → 载荷
    const payloads = windows.map(win => {
      let mx = 0
      for (let i = win.start; i < win.end; i++) mx = Math.max(mx, pqEotf(ymax[i] / 1023))
      const nits = Math.round(mx)
      const hb = nits > 0 ? Math.log2(nits / refWhite) : 0
      const raw = Math.max(0, Math.round(Math.min(6, hb) * 10000))
      return { ...win, nits, hb: +hb.toFixed(4), raw, payload: s50.t35Payload(s50.vectorReferenceWhiteRecipe(raw)) }
    })
    onProgress(0.7, 'Eclipsa：注入 2094-50 SEI…')

    // mp4 → annexb（补 AUD）→ 按 AUD 注入
    const esIn = path.join(work, 'in.h265')
    sh(ffmpeg, ['-hide_banner', '-y', '-i', inputPath, '-c', 'copy', '-bsf:v', 'hevc_mp4toannexb,hevc_metadata=aud=insert', '-f', 'hevc', esIn], path.join(work, 'annexb.log'))
    const esBuf = fs.readFileSync(esIn)
    const payloadForAu = (au) => (payloads.find(p => au >= p.start && au < p.end) || payloads[payloads.length - 1]).payload
    const injected = injectSeiPerAu(esBuf, payloadForAu)
    const esOut = path.join(work, 'injected.h265')
    fs.writeFileSync(esOut, injected)

    // remux 回 mp4（hvc1 + 避免负 PTS）+ 补回容器静态盒
    onProgress(0.9, 'Eclipsa：封装 mp4…')
    sh(ffmpeg, ['-hide_banner', '-y', '-i', esOut, '-c', 'copy', '-tag:v', 'hvc1', '-avoid_negative_ts', 'make_zero', '-movflags', '+faststart', outputPath], path.join(work, 'mux.log'))
    injectHdrBoxes(outputPath, { maxCll, maxFall })

    const nals = splitNalUnits(esBuf)
    const totalAu = nals.filter((_, i) => nalType(nals, i) === 35).length
    onProgress(1, 'Eclipsa：完成')

    return {
      outputPath,
      windows: payloads.map(p => ({
        startFrame: p.start, endFrame: p.end - 1, maxCllNits: p.nits,
        hBaseline: p.hb, raw: p.raw, payloadHex: s50.hex(p.payload)
      })),
      totalSei: totalAu,
      format: 'eclipsa'
    }
  } finally {
    try { fs.rmSync(work, { recursive: true, force: true }) } catch (e) { /* ignore */ }
  }
}

module.exports = { attachSt2094_50 }
