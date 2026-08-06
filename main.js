const { app, BrowserWindow, ipcMain, dialog } = require('electron')
const path = require('path')
const fs = require('fs')
const http = require('http')
const os = require('os')
const { spawn, exec } = require('child_process')

// 批量转换最大并发 = 核心数/2 + 1（与后端 ConversionSemaphore 一致）
const MAX_CONCURRENCY = Math.max(1, Math.floor(os.cpus().length / 2) + 1)

// ---------- Kotlin 后端管理 ----------
let backendProcess = null
let backendPort = null
let backendStarting = null

const BACKEND_JAR = path.join(__dirname, 'backend', 'kotlin', 'build', 'libs', 'hdr-converter-backend.jar')

/** 发起 HTTP JSON 请求 */
function httpJson(method, route, body) {
  return new Promise((resolve, reject) => {
    const payload = body ? JSON.stringify(body) : null
    const req = http.request({
      host: '127.0.0.1',
      port: backendPort,
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

/** 轮询 /health 直到后端 HTTP 服务真正就绪（端口行先于服务监听，必须等待） */
function waitBackendReady(port, timeoutMs = 30000) {
  const deadline = Date.now() + timeoutMs
  return new Promise((resolve, reject) => {
    const poll = async () => {
      try {
        const res = await httpJson('GET', '/health')
        if (res && res.status === 'ok') return resolve()
      } catch (e) { /* 服务未就绪，继续轮询 */ }
      if (Date.now() >= deadline) return reject(new Error('后端 HTTP 服务就绪超时'))
      setTimeout(poll, 200)
    }
    poll()
  })
}

/** 确保 Kotlin 后端已启动，返回端口 */
function ensureBackend() {
  if (backendPort && backendProcess && !backendProcess.killed) {
    return Promise.resolve(backendPort)
  }
  if (backendStarting) return backendStarting

  backendStarting = new Promise((resolve, reject) => {
    if (!fs.existsSync(BACKEND_JAR)) {
      reject(new Error('未找到后端 JAR，请先构建 Kotlin 后端:\n' + BACKEND_JAR))
      return
    }
    backendProcess = spawn('java', ['-jar', BACKEND_JAR], {
      cwd: __dirname,
      windowsHide: true
    })
    let stdout = ''
    let stderr = ''
    const timer = setTimeout(() => {
      reject(new Error('后端启动超时: ' + (stderr.slice(-300) || '无输出')))
    }, 25000)

    backendProcess.stdout.on('data', (d) => {
      stdout += d.toString()
      const m = stdout.match(/HDR_BACKEND_PORT:(\d+)/)
      if (m && !backendPort) {
        backendPort = parseInt(m[1], 10)
        clearTimeout(timer)
        // 端口行先于 HTTP 服务就绪，必须等 /health 可访问后再 resolve，否则首个请求会 ECONNREFUSED
        waitBackendReady(backendPort, 30000)
          .then(() => resolve(backendPort))
          .catch((err) => reject(err))
      }
    })
    backendProcess.stderr.on('data', (d) => { stderr += d.toString() })
    backendProcess.on('error', (err) => {
      clearTimeout(timer)
      reject(err)
    })
    backendProcess.on('exit', () => {
      backendProcess = null
      backendPort = null
      backendStarting = null
    })
  })
  return backendStarting
}

/** 轮询转换进度并转发给渲染进程 */
async function runWithProgress(sender, method, route, body) {
  await ensureBackend()
  let finished = false
  const timer = setInterval(async () => {
    if (finished) return
    try {
      const p = await httpJson('GET', '/progress')
      sender.send('conversion-progress', {
        value: parseFloat(p.value) || 0,
        active: p.active === 'true',
        message: p.message || ''
      })
    } catch (e) { /* 忽略瞬时错误 */ }
  }, 200)

  try {
    return await httpJson(method, route, body)
  } finally {
    finished = true
    clearInterval(timer)
  }
}

/** 检测是否安装了 NVIDIA GPU / CUDA（仅用于提示，实际编码走 CPU 多线程） */
function detectGpu() {
  return new Promise((resolve) => {
    exec('nvidia-smi --query-gpu=name --format=csv,noheader', { timeout: 5000 }, (err, stdout) => {
      if (err) {
        resolve({ hasGpu: false, gpuName: '' })
      } else {
        const name = (stdout || '').trim().split('\n')[0]
        resolve({ hasGpu: true, gpuName: name || 'NVIDIA GPU' })
      }
    })
  })
}

// ---------- 窗口 ----------
function createWindow() {
  const win = new BrowserWindow({
    width: 1320,
    height: 920,
    title: 'HDR Converter Electron',
    webPreferences: {
      preload: path.join(__dirname, 'preload.js'),
      nodeIntegration: false,
      contextIsolation: true,
      webSecurity: false
    }
  })

  win.loadFile(path.join(__dirname, 'hdr_viewer.html'))
}

// ---------- IPC ----------
ipcMain.handle('select-input-image', async () => {
  const { canceled, filePaths } = await dialog.showOpenDialog({
    title: '选择输入图片',
    properties: ['openFile'],
    filters: [{ name: 'Images', extensions: ['png', 'jpg', 'jpeg', 'webp', 'bmp', 'gif'] }]
  })
  if (canceled) return null
  return filePaths[0]
})

ipcMain.handle('select-input-images', async () => {
  const { canceled, filePaths } = await dialog.showOpenDialog({
    title: '选择多张输入图片',
    properties: ['openFile', 'multiSelections'],
    filters: [{ name: 'Images', extensions: ['png', 'jpg', 'jpeg', 'webp', 'bmp', 'gif'] }]
  })
  if (canceled) return null
  return filePaths
})

// 选择输入文件夹并扫描其中的图片文件
ipcMain.handle('select-input-folder', async () => {
  const { canceled, filePaths } = await dialog.showOpenDialog({
    title: '选择输入文件夹（导入其中的图片）',
    properties: ['openDirectory']
  })
  if (canceled || !filePaths[0]) return null
  const folder = filePaths[0]
  const exts = new Set(['.png', '.jpg', '.jpeg', '.webp', '.bmp', '.gif'])
  try {
    const entries = await fs.promises.readdir(folder, { withFileTypes: true })
    const files = entries
      .filter((e) => e.isFile() && exts.has(path.extname(e.name).toLowerCase()))
      .map((e) => path.join(folder, e.name))
      .sort()
    return { folder, files }
  } catch (e) {
    return { folder, files: [] }
  }
})

// 选择批量输出文件夹
ipcMain.handle('select-output-folder', async () => {
  const { canceled, filePaths } = await dialog.showOpenDialog({
    title: '选择批量输出文件夹',
    properties: ['openDirectory', 'createDirectory']
  })
  if (canceled) return null
  return filePaths[0]
})

ipcMain.handle('select-output-path', async (_event, defaultPath) => {
  const { canceled, filePath } = await dialog.showSaveDialog({
    title: '选择导出文件',
    defaultPath: defaultPath || 'hdr_output.png'
  })
  if (canceled) return null
  return filePath
})

// 读取图片文件为 base64 data URL（用于预览）
ipcMain.handle('read-image-preview', async (_event, filePath) => {
  if (!filePath) return null
  try {
    const ext = path.extname(filePath).toLowerCase().replace('.', '')
    const mimeMap = { png: 'image/png', jpg: 'image/jpeg', jpeg: 'image/jpeg', webp: 'image/webp', bmp: 'image/bmp', gif: 'image/gif' }
    const mime = mimeMap[ext] || 'image/png'
    const data = await fs.promises.readFile(filePath)
    const base64 = data.toString('base64')
    return `data:${mime};base64,${base64}`
  } catch {
    return null
  }
})

// 获取转换方式信息（供转换前展示）
ipcMain.handle('get-backend-status', async () => {
  try {
    const port = await ensureBackend()
    const [status, gpu] = await Promise.all([httpJson('GET', '/status'), detectGpu()])
    const threads = parseInt(status.threads, 10) || 0
    const capacity = parseInt(status.capacity, 10) || 0
    let message = `CPU 多线程（${threads} 核）`
    if (status.method === 'cuda') {
      // 优先用 nvidia-smi 检测到的真实 GPU 名称
      const gpuDisplay = gpu.gpuName || status.gpuName || 'NVIDIA GPU'
      message = `CUDA 加速（${gpuDisplay}）`
    }
    if (gpu.hasGpu && status.method !== 'cuda') {
      message += ` · 检测到 ${gpu.gpuName}（当前编码使用 CPU）`
    }
    return {
      method: status.method || 'cpu',
      threads,
      capacity,
      message,
      hasGpu: gpu.hasGpu,
      gpuName: gpu.gpuName
    }
  } catch (err) {
    return { method: 'cpu', threads: 0, capacity: 0, message: '后端未就绪: ' + (err.message || err), hasGpu: false, gpuName: '' }
  }
})

// 批量转换：提交任务队列，轮询 /batch/progress 转发给渲染进程
ipcMain.handle('batch-convert-images', async (event, payload) => {
  const { jobs } = payload || {}
  if (!jobs || !jobs.length) throw new Error('没有可转换的任务')
  await ensureBackend()

  const sender = event.sender
  let finished = false
  const timer = setInterval(async () => {
    if (finished) return
    try {
      const p = await httpJson('GET', '/batch/progress')
      sender.send('batch-progress', {
        total: parseInt(p.total, 10) || 0,
        done: parseInt(p.done, 10) || 0,
        failed: parseInt(p.failed, 10) || 0,
        current: p.current || '',
        message: p.message || '',
        running: (p.running === true || p.running === 'true'),
        statuses: p.statuses || {}
      })
    } catch (e) { /* 忽略瞬时错误 */ }
  }, 200)

  try {
    return await httpJson('POST', '/batch/convert', { jobs, maxConcurrent: MAX_CONCURRENCY })
  } finally {
    finished = true
    clearInterval(timer)
  }
})

// 取消批量中的指定图片（尽力而为：待处理任务直接跳过，处理中任务在阶段间中止）
ipcMain.handle('batch-cancel-images', async (event, payload) => {
  const { inputPaths } = payload || {}
  if (!inputPaths || !inputPaths.length) return { ok: true }
  await ensureBackend()
  return httpJson('POST', '/batch/cancel', { inputPaths })
})

ipcMain.handle('convert-image', async (event, payload) => {
  const { inputPath, outputPath, settings, backendPreference } = payload || {}
  if (!inputPath) throw new Error('缺少输入图片')
  return runWithProgress(event.sender, 'POST', '/convert', {
    inputPath,
    outputPath,
    settings,
    backendPreference
  })
})

// 实时预览转换（缩小尺寸快速处理）
ipcMain.handle('convert-preview', async (event, payload) => {
  const { inputPath, settings } = payload || {}
  if (!inputPath) throw new Error('缺少输入图片')
  return runWithProgress(event.sender, 'POST', '/preview', {
    inputPath,
    settings
  })
})

// ---------- 生命周期 ----------
app.whenReady().then(createWindow)

app.on('window-all-closed', () => {
  app.quit()
})

app.on('activate', () => {
  if (BrowserWindow.getAllWindows().length === 0) createWindow()
})

app.on('will-quit', () => {
  if (backendProcess) {
    try { backendProcess.kill() } catch (e) { /* ignore */ }
    backendProcess = null
    backendPort = null
  }
})
