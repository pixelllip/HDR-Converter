'use strict'
/**
 * P1 验证：输入信号解读参数化（Rust 后端 /video-frame）
 * 流程：
 *   1. ffmpeg 生成 SDR 测试帧（testsrc2，含彩条+灰阶）
 *   2. 启动 hdrconv serve（Rust 引擎）
 *   3. /video-frame（transform, peak=8）三种输入解读对照：默认(sRGB) / inputTransfer=hlg / inputPrimaries=2020
 * 断言：均返回合法 PAM；HLG、2020 解读与默认存在显著像素差异（传递函数/色域真正生效）。
 * 用法：node tests/verify_input_signal.js
 */
const fs = require('fs')
const path = require('path')
const http = require('http')
const { spawn } = require('child_process')

const FFMPEG = path.join(__dirname, '..', 'backend', 'ffmpeg', 'ffmpeg.exe')
const HDRCONV = path.join(__dirname, '..', 'backend', 'rust', 'target', 'release', 'hdrconv.exe')
const PORT = 18766
const TMP = path.join(__dirname, 'tmp_input_signal')
const FRAME_PNG = path.join(TMP, 'frame.png')

function runRaw(exe, args) {
  return new Promise((resolve, reject) => {
    const p = spawn(exe, args, { stdio: 'ignore', windowsHide: true })
    p.on('error', reject)
    p.on('close', (code) => (code === 0 ? resolve() : reject(new Error(`exit=${code}`))))
  })
}

function httpBin(port, method, route, body) {
  return new Promise((resolve, reject) => {
    const payload = body ? Buffer.from(JSON.stringify(body)) : null
    const req = http.request(
      {
        host: '127.0.0.1', port, path: route, method,
        headers: payload ? { 'Content-Type': 'application/json', 'Content-Length': payload.length } : {},
      },
      (res) => {
        const chunks = []
        res.on('data', (c) => chunks.push(c))
        res.on('end', () => resolve(Buffer.concat(chunks)))
      },
    )
    req.on('error', reject)
    if (payload) req.write(payload)
    req.end()
  })
}

async function waitHealth(port, timeoutMs = 20000) {
  const deadline = Date.now() + timeoutMs
  let last
  while (Date.now() < deadline) {
    try {
      const b = await httpBin(port, 'GET', '/health')
      if (b.toString().includes('ok')) return
    } catch (e) { last = e }
    await new Promise((r) => setTimeout(r, 200))
  }
  throw last || new Error('后端就绪超时')
}

function parsePam(pam, peak) {
  const i = pam.indexOf(Buffer.from('ENDHDR\n'))
  if (i < 0) throw new Error('PAM 无 ENDHDR 头')
  const header = pam.slice(0, i + 7).toString('latin1')
  const w = +(/WIDTH (\d+)/.exec(header) || [])[1]
  const h = +(/HEIGHT (\d+)/.exec(header) || [])[1]
  const data = pam.slice(i + 7)
  const n = w * h
  if (data.length !== n * 6) throw new Error(`PAM 数据长度 ${data.length} != ${n * 6}`)
  const lin = new Float64Array(n * 3)
  for (let k = 0; k < n; k++) {
    const o = k * 6
    lin[k * 3] = data.readUInt16BE(o) / 65535 * peak
    lin[k * 3 + 1] = data.readUInt16BE(o + 2) / 65535 * peak
    lin[k * 3 + 2] = data.readUInt16BE(o + 4) / 65535 * peak
  }
  return { w, h, lin }
}

function stat(a, b) {
  // 逐通道绝对差统计
  let max = 0, sum = 0, over005 = 0
  const n = a.length
  for (let i = 0; i < n; i++) {
    const d = Math.abs(a[i] - b[i])
    if (d > max) max = d
    sum += d
    if (d > 0.05) over005++
  }
  return { max, mean: sum / n, over005Pct: 100 * over005 / n }
}

async function videoFrame(port, extraSettings) {
  const body = {
    inputPath: FRAME_PNG,
    settings: Object.assign({ hdrIntensity: 1.8, gamma: 0.9, rgbAdjustment: { red: 1, green: 1, blue: 1 }, outputFormat: 'jpg' }, extraSettings),
    peak: 8,
    mode: 'transform',
  }
  const buf = await httpBin(port, 'POST', '/video-frame', body)
  if (buf.length < 64) throw new Error('短响应: ' + buf.toString().slice(0, 120))
  return buf
}

async function main() {
  if (!fs.existsSync(HDRCONV)) throw new Error('未找到 hdrconv.exe，请先 cargo build --release')
  if (!fs.existsSync(FFMPEG)) throw new Error('未找到 ffmpeg.exe')
  fs.mkdirSync(TMP, { recursive: true })

  console.log('1) 生成 SDR 测试帧…')
  await runRaw(FFMPEG, ['-y', '-v', 'error', '-f', 'lavfi', '-i', 'testsrc2=size=960x540:rate=1', '-frames:v', '1', '-pix_fmt', 'rgb24', FRAME_PNG])

  console.log('2) 启动 Rust 后端 serve…')
  const srv = spawn(HDRCONV, ['serve', '--port', String(PORT)], { stdio: 'ignore', windowsHide: true })
  srv.unref()
  try {
    await waitHealth(PORT)

    const base = await videoFrame(PORT, {})
    const hlgPam = await videoFrame(PORT, { inputTransfer: 'hlg' })
    const p2020 = await videoFrame(PORT, { inputPrimaries: '2020' })

    const PEAK = 8
    const a = parsePam(base, PEAK)
    const b = parsePam(hlgPam, PEAK)
    const c = parsePam(p2020, PEAK)
    const minMax = (lin) => {
      let mn = Infinity, mx = -Infinity
      for (let i = 0; i < lin.length; i++) {
        if (lin[i] < mn) mn = lin[i]
        if (lin[i] > mx) mx = lin[i]
      }
      return { mn, mx }
    }
    for (const [tag, x] of [['默认', a], ['HLG', b], ['2020', c]]) {
      const mm = minMax(x.lin)
      console.log(`  ${tag}: ${x.w}x${x.h}, 帧亮度 min/max = ${mm.mn.toFixed(3)} / ${mm.mx.toFixed(3)}`)
    }

    const d1 = stat(a.lin, b.lin) // 默认 vs HLG
    const d2 = stat(a.lin, c.lin) // 默认 vs 2020
    console.log(`默认 vs HLG : maxΔ=${d1.max.toFixed(3)} meanΔ=${d1.mean.toFixed(4)} >0.05 占比 ${d1.over005Pct.toFixed(1)}%`)
    console.log(`默认 vs 2020: maxΔ=${d2.max.toFixed(3)} meanΔ=${d2.mean.toFixed(4)} >0.05 占比 ${d2.over005Pct.toFixed(1)}%`)

    // 默认(sRGB)与 HLG/2020 应显著不同（EOTF/矩阵差），且非全黑
    const ok1 = d1.max > 0.05 && d1.mean > 0.003
    const ok2 = d2.max > 0.05 && d2.mean > 0.003
    const okRange = minMax(a.lin).mx > 1.0 // 高光扩展存在（transform 曝光=peak）
    console.log(ok1 && ok2 && okRange
      ? '✅ 输入信号解读参数化验证通过（HLG/2020 与默认显著差异；高光扩展正常）'
      : '❌ 验证失败')
    if (!(ok1 && ok2 && okRange)) process.exitCode = 1
  } finally {
    try {
      spawn('taskkill', ['/PID', String(srv.pid), '/T', '/F'], { stdio: 'ignore', windowsHide: true })
    } catch (e) { /* ignore */ }
  }
}

main().catch((e) => {
  console.error('❌ ' + (e && e.message || e))
  process.exitCode = 1
})