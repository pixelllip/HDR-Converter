'use strict'
/**
 * 验证视频侧亮度滚降（2026-11）：/video-frame 帧重建与 /preview videoDirect 共用 `rolloff`
 *
 * 断言：
 *   1. 同一纯白帧，rolloff=0/0.45/1 的白点单调递增（0=参考白、1=峰值）；
 *   2. /preview mode=videoDirect 的 PQ 码值随 rolloff 单调递增（预览与导出同参数生效）；
 *   3. 缺省（不传 rolloff）按 0.45 处理（与图片侧默认一致）。
 *
 * 用法：node tests/verify_video_rolloff.js
 */
const fs = require('fs')
const path = require('path')
const http = require('http')
const { spawn } = require('child_process')

const FFMPEG = path.join(__dirname, '..', 'backend', 'ffmpeg', 'ffmpeg.exe')
const HDRCONV = path.join(__dirname, '..', 'backend', 'rust', 'target', 'release', 'hdrconv.exe')
const PORT = 18812
const TMP = path.join(__dirname, 'tmp_video_rolloff')
const WHITE = path.join(TMP, 'white.png')

const WHITE_NITS = 203
const PEAK_NITS = 500
const EXPOSURE = PEAK_NITS / WHITE_NITS

function runRaw(exe, args) {
  return new Promise((resolve, reject) => {
    const p = spawn(exe, args, { stdio: 'ignore', windowsHide: true })
    p.on('error', reject)
    p.on('close', (code) => (code === 0 ? resolve() : reject(new Error(`exit=${code}`))))
  })
}

function httpBin(method, route, body) {
  return new Promise((resolve, reject) => {
    const payload = body ? Buffer.from(JSON.stringify(body)) : null
    const req = http.request({
      host: '127.0.0.1', port: PORT, path: route, method,
      headers: payload ? { 'Content-Type': 'application/json', 'Content-Length': payload.length } : {},
    }, (res) => {
      const chunks = []
      res.on('data', (c) => chunks.push(c))
      res.on('end', () => resolve(Buffer.concat(chunks)))
    })
    req.on('error', reject)
    if (payload) req.write(payload)
    req.end()
  })
}

async function waitHealth(timeoutMs = 20000) {
  const deadline = Date.now() + timeoutMs
  let last
  while (Date.now() < deadline) {
    try {
      const b = await httpBin('GET', '/health')
      if (b.toString().includes('ok')) return
    } catch (e) { last = e }
    await new Promise((r) => setTimeout(r, 200))
  }
  throw last || new Error('后端就绪超时')
}

/** PAM 第一像素线性值（0..1，相对 peak） */
function pamFirst(buf) {
  const i = buf.indexOf(Buffer.from('ENDHDR\n'))
  if (i < 0) throw new Error('PAM 无 ENDHDR 头')
  const o = i + 7
  return buf.readUInt16BE(o) / 65535
}

/** JPEG 第一像素的 PQ 码值（0..1，取解码后 8-bit 近似） */
async function jpegFirstCode(body) {
  const buf = await httpBin('POST', '/preview', body)
  let j
  try { j = JSON.parse(buf.toString()) } catch (e) { throw new Error('preview 响应非 JSON: ' + buf.toString().slice(0, 120)) }
  if (!j.dataUrl) throw new Error('preview 无 dataUrl: ' + JSON.stringify(j).slice(0, 160))
  const jpg = Buffer.from(j.dataUrl.split(',')[1], 'base64')
  const out = await new Promise((resolve, reject) => {
    const p = spawn(FFMPEG, ['-v', 'error', '-i', 'pipe:0', '-f', 'rawvideo', '-pix_fmt', 'rgb24', '-'],
      { stdio: ['pipe', 'pipe', 'ignore'], windowsHide: true })
    const chunks = []
    p.stdout.on('data', (d) => chunks.push(d))
    p.on('error', reject)
    p.on('close', () => resolve(Buffer.concat(chunks)))
    p.stdin.end(jpg)
  })
  return out[0] / 255
}

function videoFrameBody(rolloff) {
  const settings = {
    hdrIntensity: EXPOSURE,
    gamma: 1.0,
    rgbAdjustment: { red: 1, green: 1, blue: 1 },
    whiteNits: WHITE_NITS,
    peakNits: PEAK_NITS,
    outputFormat: 'jpg',
  }
  if (rolloff !== undefined) settings.rolloff = rolloff
  return { inputPath: WHITE, settings, peak: EXPOSURE, mode: 'transform' }
}

async function main() {
  if (!fs.existsSync(HDRCONV)) throw new Error('未找到 hdrconv.exe，请先 cargo build --release')
  if (!fs.existsSync(FFMPEG)) throw new Error('未找到 ffmpeg.exe')
  fs.mkdirSync(TMP, { recursive: true })
  await runRaw(FFMPEG, ['-y', '-v', 'error', '-f', 'lavfi', '-i', 'color=c=0xffffff:size=64x64:rate=1', '-frames:v', '1', '-pix_fmt', 'rgb24', WHITE])

  console.log('启动 Rust 后端 serve…')
  const server = spawn(HDRCONV, ['serve', '--port', String(PORT)], { windowsHide: true, stdio: 'ignore' })
  try {
    await waitHealth()
    const vals = {}
    for (const r of [0, 0.45, 1]) {
      const pam = await httpBin('POST', '/video-frame', videoFrameBody(r))
      vals[r] = pamFirst(pam)
      console.log(`/video-frame rolloff=${r}: 白点 PAM=${vals[r].toFixed(4)}（理论 ${(1 + (EXPOSURE - 1) * r).toFixed(4)}/${EXPOSURE.toFixed(4)}=${((1 + (EXPOSURE - 1) * r) / EXPOSURE).toFixed(4)}）`)
    }
    const def = pamFirst(await httpBin('POST', '/video-frame', videoFrameBody(undefined)))
    console.log(`/video-frame 缺省: 白点 PAM=${def.toFixed(4)}（应=rolloff 0.45）`)

    const previews = {}
    for (const r of [0, 0.45, 1]) {
      previews[r] = await jpegFirstCode({ inputPath: WHITE, settings: videoFrameBody(r).settings, mode: 'videoDirect' })
      console.log(`/preview rolloff=${r}: 白点 PQ 码值=${previews[r].toFixed(4)}`)
    }

    const mono = vals[0] < vals[0.45] && vals[0.45] < vals[1]
    const monoPreview = previews[0] < previews[0.45] && previews[0.45] < previews[1]
    const whiteOk = Math.abs(vals[0] - 1 / EXPOSURE) < 0.01 && Math.abs(vals[1] - 1.0) < 0.01
    const defaultOk = Math.abs(def - vals[0.45]) < 0.005
    const ok = mono && monoPreview && whiteOk && defaultOk
    console.log(ok ? '\n✅ 视频亮度滚降生效（导出/预览同参数，白点 0→参考白、1→峰值）' : '\n❌ 视频亮度滚降异常')
    process.exitCode = ok ? 0 : 1
  } finally {
    server.kill()
  }
}

main().catch((e) => { console.error('❌', e.message); process.exit(1) })
