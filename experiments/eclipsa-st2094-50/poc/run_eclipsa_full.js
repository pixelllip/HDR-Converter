'use strict'
/**
 * run_eclipsa_full.js — 第三格式端到端：原始 SDR MKV → 逐帧增益图 HDR10 → 附加 ST 2094-50(Eclipsa)
 * 走产品代码：video_converter.convertVideoFrames(..., { backendPort, transformMode:'gainmap', format:'eclipsa' })
 */
const { spawn } = require('child_process')
const http = require('http')
const path = require('path')
const fs = require('fs')

const ROOT = path.resolve(__dirname, '..', '..', '..')
// Kotlin 已存档（archive/kotlin-backend/）：Rust 优先，缺失时回退存档 jar
const JAR = path.join(ROOT, 'archive', 'kotlin-backend', 'build', 'libs', 'hdr-converter-backend.jar')
const RUST_EXE = path.join(ROOT, 'backend', 'rust', 'target', 'release', 'hdrconv.exe')
const VC = require(path.join(ROOT, 'video_converter.js'))

const INPUT = 'D:/video/video_sdr/bg_waifu2x_2x_2n_mp4.mkv'
// 用法: node run_eclipsa_full.js [scene|uniform] [uniformWindows]
const SCHEME = (process.argv[2] || 'scene').toLowerCase()
const UNIFORM_WIN = parseInt(process.argv[3] || '3', 10) || 3
const OUT_TAG = (SCHEME === 'uniform' ? ('u' + UNIFORM_WIN) : 's')
const OUTPUT = 'D:/video/video_sdr/bg_waifu2x_2x_2n_mp4_Eclipsa_' + OUT_TAG + '.mp4'

function findJava() {
  const jdk21 = 'C:\\Users\\Administrator\\.gradle\\jdks\\jetbrains_s_r_o_-21-amd64-windows.2\\bin\\java.exe'
  return fs.existsSync(jdk21) ? jdk21 : 'java'
}
function httpGet(url) { return new Promise((res, rej) => http.get(url, r => { let d = ''; r.on('data', c => d += c); r.on('end', () => res(d)) }).on('error', rej)) }
const sleep = ms => new Promise(r => setTimeout(r, ms))

async function startBackend() {
  // Kotlin 已存档：Rust 引擎优先，缺失时回退存档 jar
  const useRust = fs.existsSync(RUST_EXE)
  if (!useRust && !fs.existsSync(JAR)) throw new Error('后端引擎均不存在: ' + RUST_EXE + ' / ' + JAR)
  const child = useRust
    ? spawn(RUST_EXE, ['serve', '--port', '0'], { cwd: ROOT, windowsHide: true })
    : spawn(findJava(), ['-jar', JAR], { cwd: ROOT, windowsHide: true })
  let logs = '', port = null
  child.stdout.setEncoding('utf8'); child.stderr.setEncoding('utf8')
  child.stdout.on('data', d => { logs += d; const m = logs.match(/HDR_BACKEND_PORT:(\d+)/); if (m && !port) port = parseInt(m[1], 10) })
  child.stderr.on('data', d => { /* backend logs */ })
  const t0 = Date.now()
  while (!port && Date.now() - t0 < 30000 && child.exitCode === null) await sleep(100)
  if (!port) { try { child.kill() } catch (e) {} ; throw new Error('后端端口未出现') }
  for (let i = 0; i < 60; i++) { try { await httpGet('http://127.0.0.1:' + port + '/health'); break } catch (e) { await sleep(500) } }
  return { port, stop() { try { child.kill() } catch (e) {} } }
}

async function main() {
  const whiteNits = 203, peakNits = 574
  const settings = {
    hdrIntensity: Math.log2(peakNits / whiteNits), // 增益图 EV
    gamma: 1.0, fineTuneBrightness: 1.0,
    rgbAdjustment: { red: 1, green: 1, blue: 1 },
    whiteNits, peakNits, crf: 20, maxWidth: 0, encoder: 'x265',
    format: 'eclipsa', // ★ 第三种格式
    eclipsa: { windowScheme: SCHEME, uniformWindows: UNIFORM_WIN } // 参考白默认跟随链路白点(203)
  }
  console.log('输入 :', INPUT)
  console.log('输出 :', OUTPUT)
  console.log('格式 : Eclipsa Video（HDR10 + ST 2094-50 动态）| 分窗=' + SCHEME +
    ' | 参考白=跟随链路白点(203) | 每窗数(uniform)=' + UNIFORM_WIN)

  const backend = await startBackend()
  console.log('后端就绪 port=', backend.port)
  try {
    const result = await VC.convertVideoFrames(INPUT, OUTPUT, settings,
      { backendPort: backend.port, transformMode: 'gainmap', format: 'eclipsa', eclipsaOpts: settings.eclipsa },
      (v, m) => { if (m) process.stdout.write('[progress] ' + (v * 100).toFixed(1) + '% ' + m + '\n') })
    console.log('\n结果:', JSON.stringify({
      outputPath: result.outputPath,
      encoder: result.encoder,
      format: result.info && result.info.format,
      eclipsa: result.info && result.info.eclipsa
    }, null, 1))
  } finally { backend.stop() }
}

main().catch(e => { console.error('失败:', e); process.exit(1) })
