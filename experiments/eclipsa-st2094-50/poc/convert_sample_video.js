'use strict'
/**
 * convert_sample_video.js — 直接用本应用的真实转换链路转一段视频（不经 GUI）
 * 用法: node convert_sample_video.js <input> [mode] [output]
 *   mode: frames(默认·逐帧增益图) | direct(单层色调映射)
 * 依赖: 项目自带 ffmpeg 9.0 + backend/kotlin JAR（自动启动后端并等 /health）
 */
const { spawn } = require('child_process')
const http = require('http')
const path = require('path')
const fs = require('fs')

const ROOT = path.resolve(__dirname, '..', '..', '..') // hdr_electron
const JAR = path.join(ROOT, 'backend', 'kotlin', 'build', 'libs', 'hdr-converter-backend.jar')
const VC = require(path.join(ROOT, 'video_converter.js'))

const INPUT = process.argv[2]
const MODE = (process.argv[3] || 'frames').toLowerCase()
if (!INPUT) { console.error('用法: node convert_sample_video.js <input> [frames|direct] [output]'); process.exit(2) }
const ext = path.extname(INPUT) || '.mkv'
const OUTPUT = process.argv[4] || path.join(path.dirname(INPUT), `${path.basename(INPUT, ext)}_HDR10_${MODE}.mp4`)

function findJava() {
  const jdk21 = 'C:\\Users\\Administrator\\.gradle\\jdks\\jetbrains_s_r_o_-21-amd64-windows.2\\bin\\java.exe'
  if (fs.existsSync(jdk21)) return jdk21
  return 'java'
}
function httpGet(url) {
  return new Promise((res, rej) => {
    http.get(url, r => { let d = ''; r.on('data', c => d += c); r.on('end', () => res(d)) }).on('error', rej)
  })
}
function sleep(ms) { return new Promise(r => setTimeout(r, ms)) }

async function startBackend() {
  if (!fs.existsSync(JAR)) throw new Error('后端 JAR 不存在: ' + JAR)
  const child = spawn(findJava(), ['-jar', JAR], { cwd: ROOT, windowsHide: true })
  let logs = '', port = null
  child.stdout.setEncoding('utf8'); child.stderr.setEncoding('utf8')
  child.stdout.on('data', d => {
    logs += d
    process.stdout.write('[backend] ' + d)
    const m = logs.match(/HDR_BACKEND_PORT:(\d+)/)
    if (m && !port) port = parseInt(m[1], 10)
  })
  child.stderr.on('data', d => process.stdout.write('[backend!] ' + d))
  // 等端口行（最多 30s）
  const t0 = Date.now()
  while (!port && Date.now() - t0 < 30000 && child.exitCode === null) await sleep(100)
  if (!port) { try { child.kill() } catch (e) {} ; throw new Error('后端端口行未出现') }
  // 等 /health（HTTP 服务就绪）
  for (let i = 0; i < 60; i++) {
    try { await httpGet('http://127.0.0.1:' + port + '/health'); break } catch (e) { await sleep(500) }
  }
  return {
    port,
    stop() { try { child.kill() } catch (e) {} }
  }
}

async function main() {
  if (!fs.existsSync(INPUT)) throw new Error('输入不存在: ' + INPUT)
  const whiteNits = 203
  const peakNits = 574 // = 203×2^1.5，项目默认峰值
  const settings = {
    hdrIntensity: MODE === 'direct' ? (peakNits / whiteNits) : Math.log2(peakNits / whiteNits),
    gamma: 1.0,
    fineTuneBrightness: 1.0,
    rgbAdjustment: { red: 1.0, green: 1.0, blue: 1.0 },
    whiteNits,
    peakNits,
    crf: 20,
    maxWidth: 0,       // 保持源 4K 分辨率
    encoder: 'x265'    // CPU 软编 HEVC（coded==visible）
  }
  console.log('输入  :', INPUT)
  console.log('输出  :', OUTPUT)
  console.log('模式  :', MODE, ' EV=', settings.hdrIntensity.toFixed(3),
    ' 参考白=', whiteNits, ' 峰值=', peakNits, ' crf=', settings.crf)

  const backend = await startBackend()
  console.log('后端已就绪 port=', backend.port)
  try {
    const onProgress = (v, m) => { if (m) process.stdout.write('[progress] ' + (v * 100).toFixed(1) + '% ' + m + '\n') }
    const result = MODE === 'direct'
      ? await VC.convertVideoDirect(INPUT, OUTPUT, settings, { backendPort: backend.port }, onProgress)
      : await VC.convertVideoFrames(INPUT, OUTPUT, settings, { backendPort: backend.port, transformMode: 'gainmap' }, onProgress)
    console.log('\n转换完成:', JSON.stringify(result))
    console.log('输出文件:', result.outputPath)
  } finally {
    backend.stop()
  }
}

main().catch(err => { console.error('转换失败:', err); process.exit(1) })
