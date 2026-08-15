/**
 * 测试辅助：启动 Kotlin 后端并通过 HTTP 调用 /convert
 *
 * JS 后端已移除（后端完全由 Kotlin 接管），验证脚本统一通过本模块
 * 拉起 `backend/kotlin/build/libs/hdr-converter-backend.jar` 并调用其 HTTP 接口。
 */
const http = require('http')
const { spawn } = require('child_process')
const path = require('path')
const fs = require('fs')

const JAR = path.join(__dirname, '..', 'backend', 'kotlin', 'build', 'libs', 'hdr-converter-backend.jar')

let proc = null
let port = null
let ready = null

function httpJson(method, route, body) {
  return new Promise((resolve, reject) => {
    const payload = body ? JSON.stringify(body) : null
    const req = http.request({
      host: '127.0.0.1',
      port,
      path: route,
      method,
      headers: payload
        ? { 'Content-Type': 'application/json', 'Content-Length': Buffer.byteLength(payload) }
        : {}
    }, (res) => {
      let data = ''
      res.on('data', (c) => (data += c))
      res.on('end', () => {
        try {
          resolve(JSON.parse(data))
        } catch (e) {
          reject(new Error('后端响应解析失败: ' + String(data).slice(0, 200)))
        }
      })
    })
    req.on('error', reject)
    if (payload) req.write(payload)
    req.end()
  })
}

/** 轮询 /health 直到后端 HTTP 服务真正就绪 */
async function waitReady(timeoutMs = 30000) {
  const deadline = Date.now() + timeoutMs
  let lastErr = null
  while (Date.now() < deadline) {
    try {
      const res = await httpJson('GET', '/health')
      if (res && res.status === 'ok') return
      lastErr = new Error('health 异常: ' + JSON.stringify(res))
    } catch (e) {
      lastErr = e
    }
    await new Promise((r) => setTimeout(r, 200))
  }
  throw lastErr || new Error('后端就绪超时')
}

/** 确保 Kotlin 后端已启动并 HTTP 就绪，返回端口 */
function ensureBackend() {
  if (proc && !proc.killed && port) return ready
  ready = new Promise((resolve, reject) => {
    if (!fs.existsSync(JAR)) {
      reject(new Error('未找到后端 JAR，请先构建 Kotlin 后端:\n' + JAR))
      return
    }
    const p = spawn('java', ['-jar', JAR], { cwd: __dirname, windowsHide: true })
    proc = p
    // unref：避免 java 子进程拖住 Node 事件循环，导致脚本不退出 / 后端进程泄漏
    p.unref()
    let stdout = ''
    let stderr = ''
    const timer = setTimeout(() => {
      reject(new Error('后端启动超时: ' + (stderr.slice(-300) || '无输出')))
    }, 60000)

    p.stdout.on('data', (d) => {
      stdout += d.toString()
      const m = stdout.match(/HDR_BACKEND_PORT:(\d+)/)
      if (m && !port) {
        port = parseInt(m[1], 10)
        clearTimeout(timer)
        // 端口行先于 HTTP 服务就绪，需等 /health 可访问
        waitReady()
          .then(() => resolve(port))
          .catch((err) => reject(err))
      }
    })
    p.stderr.on('data', (d) => { stderr += d.toString() })
    p.on('error', (err) => {
      clearTimeout(timer)
      reject(err)
    })
    p.on('exit', () => {
      // 仅当退出的仍是当前进程时才清状态，否则旧进程 exit 会误清新启动后端的状态
      if (proc === p) {
        proc = null
        port = null
        ready = null
      }
    })
  })
  return ready
}

/**
 * 调用 Kotlin 后端 /convert（等价于旧的 JS convertImage）
 *
 * @param {object} opts
 * @param {string} opts.inputPath
 * @param {string} opts.outputPath
 * @param {object} [opts.settings]  { hdrIntensity, fineTuneBrightness, gamma, outputFormat, rgbAdjustment }
 * @returns {Promise<{success, outputPath, outputFormat, message}>}
 */
async function convertImage({ inputPath, outputPath, settings = {} }) {
  await ensureBackend()
  const res = await httpJson('POST', '/convert', { inputPath, outputPath, settings })
  if (!res.success) {
    throw new Error(res.message || '转换失败')
  }
  return res
}

/**
 * 关闭 Kotlin 后端（避免测试退出后遗留 java 进程）
 *
 * 注意：Windows 上 child.kill() 只返回 true，并不真正终止 java.exe，
 * 必须用 taskkill /T /F 强制终止进程树。
 */
function stopBackend() {
  // 同步重置缓存：否则下一次 ensureBackend() 会拿到已死后端的旧 ready/port（ECONNREFUSED），
  // 且 HDR_GPU_DISABLE 等环境变量对新启动的后端才生效
  const p = proc
  proc = null
  port = null
  ready = null
  if (p && !p.killed) {
    if (process.platform === 'win32' && p.pid) {
      spawn('taskkill', ['/PID', String(p.pid), '/T', '/F'], { windowsHide: true, stdio: 'ignore' })
    } else {
      p.kill()
    }
  }
}

// 无论正常/异常退出都清理后端进程
process.on('exit', () => {
  if (proc && !proc.killed && !stopping) stopBackend()
})

module.exports = { convertImage, ensureBackend, stopBackend, httpJson }
