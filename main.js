const { app, BrowserWindow, ipcMain, dialog, shell } = require('electron')
const path = require('path')
const fs = require('fs')
const http = require('http')
const os = require('os')
const { spawn, exec } = require('child_process')
const videoConverter = require('./video_converter')

// 修复 Windows PowerShell/CMD 默认 GBK 终端下后端中文日志乱码：
// Java 子进程用 UTF-8 输出，父进程按 UTF-8 解码后转发。
if (process.platform === 'win32') {
  try {
    process.env.PYTHONIOENCODING = 'utf-8'
    if (!process.env.JAVA_TOOL_OPTIONS) {
      process.env.JAVA_TOOL_OPTIONS = '-Dfile.encoding=UTF-8 -Dstdout.encoding=UTF-8 -Dstderr.encoding=UTF-8'
    }
  } catch (e) { /* ignore */ }
}

// 批量转换最大并发 = 核心数/2 + 1（与后端 ConversionSemaphore 一致）
const MAX_CONCURRENCY = Math.max(1, Math.floor(os.cpus().length / 2) + 1)

// ---------- Kotlin 后端管理 ----------
let backendProcess = null
let backendPort = null
let backendStarting = null

const BACKEND_JAR_DIR_REL = path.join('backend', 'kotlin', 'build', 'libs')

/**
 * 解析真实的 java.exe（避免 Oracle javapath 启动器：它会再拉一个真正的 JVM 子进程，
 * 导致 child.kill()/stdin 管道都只作用于启动器，正常退出也会遗留孤儿 JVM）。
 * 优先 JAVA_HOME/bin/java.exe，其次 PATH 中非 javapath 的 java.exe，最后回退 'java'。
 */
function resolveJavaExecutable() {
  const candidates = []
  if (process.env.JAVA_HOME) {
    candidates.push(path.join(process.env.JAVA_HOME, 'bin', 'java.exe'))
  }
  const pathDirs = (process.env.PATH || '').split(path.delimiter)
  for (const dir of pathDirs) {
    if (!dir || /javapath/i.test(dir)) continue
    candidates.push(path.join(dir, 'java.exe'))
  }
  candidates.push('java')
  for (const c of candidates) {
    try {
      if (c === 'java' || fs.existsSync(c)) return c
    } catch (e) { /* ignore */ }
  }
  return 'java'
}

const JAVA_EXE = resolveJavaExecutable()

// 打包后：被外部进程（java）读取的 JAR 在 resources/app.asar.unpacked。
// 不能用 existsSync 判断——asar 里即使只有 stub/副本 existsSync 也可能为 true，但 spawn/System.load 打不开。
// 用 app.isPackaged 确定性选择：打包 → 解包目录；开发(npm start) → __dirname。
// 注意：spawn 的 cwd 必须是真实目录（打包后 __dirname 是 app.asar 文件 → ENOENT），用 resourcesPath。
function resourcePath(rel) {
  if (app.isPackaged) return path.join(process.resourcesPath, 'app.asar.unpacked', rel)
  return path.join(__dirname, rel)
}

const MAIN_CWD = app.isPackaged ? process.resourcesPath : __dirname

const BACKEND_JAR = resourcePath(path.join(BACKEND_JAR_DIR_REL, 'hdr-converter-backend.jar'))

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
    // 防止请求永久挂起（后端冷启动时 connect 可能阻塞很久）
    req.setTimeout(30000, () => {
      req.destroy(new Error('后端请求超时(30s): ' + method + ' ' + route))
    })
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
    backendProcess = spawn(JAVA_EXE, ['-jar', BACKEND_JAR], {
      cwd: MAIN_CWD,
      windowsHide: true
    })
    // 显式按 UTF-8 解码后端子进程的字节流，避免 Windows 终端按 GBK 显示中文乱码
    if (backendProcess.stdout && typeof backendProcess.stdout.setEncoding === 'function') {
      backendProcess.stdout.setEncoding('utf8')
    }
    if (backendProcess.stderr && typeof backendProcess.stderr.setEncoding === 'function') {
      backendProcess.stderr.setEncoding('utf8')
    }
    let stdout = ''
    let stderr = ''
    const timer = setTimeout(() => {
      reject(new Error('后端启动超时: ' + (stderr.slice(-300) || '无输出')))
    }, 25000)

    backendProcess.stdout.on('data', (d) => {
      const text = d.toString()
      stdout += text
      // 转发后端日志到主进程控制台（便于排查 GPU/CUDA 状态等）
      process.stdout.write('[backend] ' + text)
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
    backendProcess.stderr.on('data', (d) => {
      const text = d.toString()
      stderr += text
      // 后端 stderr（含 [HdrGpuJni] CUDA 状态/回退日志）转发到主进程控制台
      process.stderr.write('[backend] ' + text)
    })
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

/** 用 nvidia-smi 获取真实 GPU 型号用于界面展示（后端 /status 的 gpuName 只是固定字符串）。
 *  实际编码由后端自动选择：GPU/CUDA 可用时走 CUDA（nativeApplyHdrTransform），失败才回退 CPU。 */
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

/**
 * 启动时清理历史遗留的孤儿后端 JVM（旧版本崩溃/强杀残留，命令行匹配 hdr-converter-backend）。
 * 在 createWindow 之前 await，确保不会误杀本实例随后拉起的后端。
 */
function sweepOrphanBackends() {
  return new Promise((resolve) => {
    const script =
      "Get-CimInstance Win32_Process -Filter \"Name='java.exe'\" | " +
      "Where-Object { $_.CommandLine -like '*hdr-converter-backend*' } | " +
      "ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }"
    try {
      exec(`powershell -NoProfile -NonInteractive -Command "${script}"`, { timeout: 15000, windowsHide: true }, () => resolve())
    } catch (e) {
      resolve()
    }
  })
}

// ---------- 窗口 ----------
function createWindow() {
  // 打包后资源在 app.asar.unpacked；开发模式读 __dirname/assets
  // Windows 任务栏/Dock/Alt-Tab 都依赖这里，缺失就显示默认 Electron 图标
  const iconPath = app.isPackaged
    ? path.join(process.resourcesPath, 'app.asar.unpacked', 'assets', 'logo.ico')
    : path.join(__dirname, 'assets', 'logo.ico')

  const win = new BrowserWindow({
    width: 1320,
    height: 920,
    // 防止缩到布局崩溃（392px 参数面板 + 预览区最小可用宽度）
    minWidth: 1000,
    minHeight: 680,
    title: 'HDR Converter',
    icon: iconPath,
    // 与深色主题 --surface 一致，避免启动时白闪
    backgroundColor: '#141218',
    autoHideMenuBar: true,
    webPreferences: {
      preload: path.join(__dirname, 'preload.js'),
      nodeIntegration: false,
      contextIsolation: true,
      webSecurity: false
    }
  })

  // 开发模式热重载：保存前端文件后窗口自动刷新（打包后不启用）
  if (!app.isPackaged) setupDevReload(win)

  win.loadFile(path.join(__dirname, 'views', 'home.html'))
}

// 开发模式：监听 views/（home·image·video + md3.css/js）与 preload.js 变化，防抖后刷新窗口，并自动弹出独立 DevTools。
// 监听整个目录而非单个文件，兼容编辑器"先写临时文件再改名"的原子保存方式。
// 注意：转换进行中保存会刷新界面状态（后端任务不受影响）；改 main.js / video_converter.js 仍需重启应用。
function setupDevReload(win) {
  const watched = new Set(['home.html', 'image.html', 'video.html', 'md3.css', 'md3.js', 'preload.js'])
  let reloadTimer = null
  let devToolsOpened = false

  const watcher = fs.watch(path.join(__dirname, 'views'), { interval: 200 }, (_eventType, filename) => {
    if (!filename || !watched.has(filename)) return
    if (reloadTimer) clearTimeout(reloadTimer)
    reloadTimer = setTimeout(() => {
      if (!win.isDestroyed()) win.webContents.reload()
    }, 150)
  })

  // preload.js 变化也会触发刷新（主目录级别，另加一个 watcher）
  const preloadWatcher = fs.watch(__dirname, { interval: 200 }, (_eventType, filename) => {
    if (filename !== 'preload.js') return
    if (reloadTimer) clearTimeout(reloadTimer)
    reloadTimer = setTimeout(() => {
      if (!win.isDestroyed()) win.webContents.reload()
    }, 150)
  })

  win.webContents.on('did-finish-load', () => {
    if (!devToolsOpened && !win.isDestroyed()) {
      devToolsOpened = true
      win.webContents.openDevTools({ mode: 'detach' })
    }
  })

  // 关闭主窗口时先关掉 detached DevTools 窗口：
  // DevTools 是独立 BrowserWindow，不关掉会让 window-all-closed 不触发 → app 不退出 → 后端 JVM 残留。
  win.on('close', () => {
    if (!win.isDestroyed()) {
      try { win.webContents.closeDevTools() } catch (e) { /* ignore */ }
    }
  })

  win.on('closed', () => {
    if (reloadTimer) clearTimeout(reloadTimer)
    watcher.close()
    preloadWatcher.close()
  })

  console.log('[dev] 热重载已启用：保存 views/（home·image·video + md3.css/js）或 preload.js 后窗口自动刷新')
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

// 首页统一选择：图片或视频（渲染进程按扩展名自动路由到对应页面）
ipcMain.handle('select-input-file', async () => {
  const { canceled, filePaths } = await dialog.showOpenDialog({
    title: '选择图片或视频',
    properties: ['openFile'],
    filters: [
      { name: '图片 / 视频', extensions: ['png', 'jpg', 'jpeg', 'webp', 'bmp', 'gif', 'mp4', 'mkv', 'mov', 'avi', 'webm', 'm4v', 'ts', 'mts'] },
      { name: '图片', extensions: ['png', 'jpg', 'jpeg', 'webp', 'bmp', 'gif'] },
      { name: '视频', extensions: ['mp4', 'mkv', 'mov', 'avi', 'webm', 'm4v', 'ts', 'mts'] }
    ]
  })
  if (canceled) return null
  return filePaths[0]
})

// 在资源管理器中显示输出文件/文件夹（文件不存在时打开其所在目录）
ipcMain.handle('open-output-folder', async (_event, filePath) => {
  if (!filePath) return { ok: false, message: '没有可打开的路径' }
  try {
    if (fs.existsSync(filePath)) {
      shell.showItemInFolder(filePath)
      return { ok: true }
    }
    const dir = path.dirname(filePath)
    const target = fs.existsSync(dir) ? dir : path.dirname(dir)
    const err = await shell.openPath(target)
    return err ? { ok: false, message: err } : { ok: true }
  } catch (e) {
    return { ok: false, message: e && e.message ? e.message : String(e) }
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

// 取消当前单张图片转换（尽力而为：后端在阶段间中止，返回 success=false message=已取消）
ipcMain.handle('cancel-image', async () => {
  await ensureBackend()
  return httpJson('POST', '/cancel', {})
})

// 实时预览转换（缩小尺寸快速处理）
ipcMain.handle('convert-preview', async (event, payload) => {
  const { inputPath, settings, mode } = payload || {}
  if (!inputPath) throw new Error('缺少输入图片')
  return runWithProgress(event.sender, 'POST', '/preview', {
    inputPath,
    settings,
    mode
  })
})

// 自动估算 HDR 强度（亮度直方图分析，返回建议 EV；前端换算为峰值亮度（尼特）显示）
ipcMain.handle('estimate-hdr-intensity', async (event, payload) => {
  const { inputPath } = payload || {}
  if (!inputPath) throw new Error('缺少输入图片')
  await ensureBackend()
  return httpJson('POST', '/estimate', { inputPath })
})

// 读取显示器峰值亮度（DXGI_OUTPUT_DESC1.MaxLuminance，单位尼特）。
// SDR 屏返回 0；HDR 屏返回厂商报告的峰值（如 1000）；读取失败返回 null。
// 脚本必须保持纯 ASCII（PowerShell 5.1 无 BOM 时按 ANSI 读取 UTF-8 中文会破坏 here-string）。
const DISPLAY_LUM_PS = `# Read display peak luminance (DXGI_OUTPUT_DESC1.MaxLuminance, nits; SDR -> 0)
$code = @"
using System;
using System.Runtime.InteropServices;

public static class DxgiLum {
    [StructLayout(LayoutKind.Sequential)]
    public struct DXGI_RATIONAL {
        public int Numerator;
        public int Denominator;
    }

    [StructLayout(LayoutKind.Sequential)]
    public struct DXGI_OUTPUT_DESC1 {
        public int Left, Top, Right, Bottom;
        public int Rotation;
        public DXGI_RATIONAL RedPrimary, GreenPrimary, BluePrimary, WhitePoint;
        public float MinLuminance, MaxLuminance, MaxFullFrameLuminance;
        public int BitsPerColor;
        public int ColorSpace;
        public int Flags;
    }

    [DllImport("dxgi.dll")]
    public static extern int CreateDXGIFactory1(ref Guid riid, out IntPtr ppFactory);

    [UnmanagedFunctionPointer(CallingConvention.StdCall)]
    public delegate int EnumAdapters1Del(IntPtr factory, int adapterIndex, out IntPtr adapter);
    [UnmanagedFunctionPointer(CallingConvention.StdCall)]
    public delegate int EnumOutputsDel(IntPtr adapter, int outputIndex, out IntPtr output);
    [UnmanagedFunctionPointer(CallingConvention.StdCall)]
    public delegate int GetDesc1Del(IntPtr output, ref DXGI_OUTPUT_DESC1 desc);

    public static double GetMaxLuminance() {
        try {
            Guid iid = new Guid("770aad78-f26f-4dba-a829-253c83d1b387"); // IDXGIFactory1
            IntPtr factory;
            int hr = CreateDXGIFactory1(ref iid, out factory);
            if (hr != 0 || factory == IntPtr.Zero) return 0;
            try {
                IntPtr fv = Marshal.ReadIntPtr(factory);
                // IDXGIFactory1.EnumAdapters1 = vtable slot 12
                EnumAdapters1Del enumAdapters = (EnumAdapters1Del)Marshal.GetDelegateForFunctionPointer(Marshal.ReadIntPtr(fv, 12 * IntPtr.Size), typeof(EnumAdapters1Del));
                IntPtr adapter = IntPtr.Zero;
                for (int ai = 0; ; ai++) {
                    if (enumAdapters(factory, ai, out adapter) != 0 || adapter == IntPtr.Zero) break;
                    try {
                        IntPtr av = Marshal.ReadIntPtr(adapter);
                        // IDXGIAdapter.EnumOutputs = vtable slot 7
                        EnumOutputsDel enumOutputs = (EnumOutputsDel)Marshal.GetDelegateForFunctionPointer(Marshal.ReadIntPtr(av, 7 * IntPtr.Size), typeof(EnumOutputsDel));
                        IntPtr output = IntPtr.Zero;
                        for (int oi = 0; ; oi++) {
                            if (enumOutputs(adapter, oi, out output) != 0 || output == IntPtr.Zero) break;
                            try {
                                IntPtr ov = Marshal.ReadIntPtr(output);
                                // IDXGIOutput6.GetDesc1 = vtable slot 27
                                GetDesc1Del getDesc1 = (GetDesc1Del)Marshal.GetDelegateForFunctionPointer(Marshal.ReadIntPtr(ov, 27 * IntPtr.Size), typeof(GetDesc1Del));
                                DXGI_OUTPUT_DESC1 desc = new DXGI_OUTPUT_DESC1();
                                if (getDesc1(output, ref desc) == 0 && desc.MaxLuminance > 0) {
                                    return (double)desc.MaxLuminance;
                                }
                            } finally { Marshal.Release(output); }
                        }
                    } finally { Marshal.Release(adapter); }
                }
            } finally { Marshal.FinalReleaseComObject(Marshal.GetObjectForIUnknown(factory)); }
            return 0;
        } catch { return 0; }
    }
}
"@
Add-Type -TypeDefinition $code -ErrorAction Stop
$n = [DxgiLum]::GetMaxLuminance()
if ($n -gt 0) { Write-Output ("NITS=" + [math]::Round($n)) } else { Write-Output "NITS=0" }`

// 读取显示器峰值亮度（DXGI_OUTPUT_DESC1.MaxLuminance，单位尼特）。
// 返回 { nits }：nits>0 = HDR 屏报告的真实峰值；nits=0 = SDR 屏（报告 0）；
// 读取失败（脚本异常/超时/无 powershell）返回 null。
// 结果缓存 60 秒（显示器参数不会频繁变化，避免每次估算都重新编译 Add-Type）。
let displayLumCache = { t: 0, nits: null, failed: true }
ipcMain.handle('get-display-peak-luminance', async () => {
  const now = Date.now()
  if (now - displayLumCache.t < 60000) return displayLumCache.failed ? null : { nits: displayLumCache.nits }
  const psFile = path.join(os.tmpdir(), 'hdr_display_lum_' + process.pid + '.ps1')
  try {
    fs.writeFileSync(psFile, DISPLAY_LUM_PS, 'utf8')
  } catch {
    displayLumCache = { t: now, nits: null, failed: true }
    return null
  }
  try {
    const stdout = await new Promise((resolve) => {
      exec('powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "' + psFile + '"', { timeout: 8000, windowsHide: true }, (err, out) => resolve(String(out || '')))
    })
    const m = stdout.match(/NITS=(\d+)/)
    const nits = m ? parseInt(m[1], 10) : 0
    displayLumCache = { t: now, nits, failed: false }
    console.log('[display-luminance] DXGI 报告峰值亮度 = ' + nits + ' 尼特' + (nits > 0 ? '（HDR 屏）' : '（SDR 屏）'))
    return { nits }
  } catch (err) {
    console.log('[display-luminance] 读取失败: ' + (err && err.message))
    displayLumCache = { t: now, nits: null, failed: true }
    return null
  } finally {
    try { fs.unlinkSync(psFile) } catch { /* ignore */ }
  }
})

// ---------- 视频转换（ffmpeg） ----------
let currentVideoCanceled = false

// 选择输入视频
ipcMain.handle('select-input-video', async () => {
  const { canceled, filePaths } = await dialog.showOpenDialog({
    title: '选择输入视频',
    properties: ['openFile'],
    filters: [{ name: 'Videos', extensions: ['mp4', 'mkv', 'mov', 'avi', 'webm', 'm4v', 'ts', 'mts'] }]
  })
  if (canceled) return null
  return filePaths[0]
})

// 选择 HDR 视频导出路径
ipcMain.handle('select-output-video', async (_event, defaultPath) => {
  const { canceled, filePath } = await dialog.showSaveDialog({
    title: '选择 HDR 视频导出路径',
    defaultPath: defaultPath || 'hdr_output.mp4',
    filters: [{ name: 'MP4 (HDR10)', extensions: ['mp4'] }]
  })
  if (canceled) return null
  return filePath
})

// 探测视频信息（时长/帧率/分辨率/音频）
ipcMain.handle('probe-video', async (_event, inputPath) => {
  if (!inputPath) throw new Error('缺少输入视频')
  return videoConverter.probeVideo(inputPath)
})

// 视频转换：mode = 'direct'（单层色调映射，图片 ICC 增益式）| 'frames'（逐帧增益图）
ipcMain.handle('convert-video', async (event, payload) => {
  const { inputPath, outputPath, settings, mode } = payload || {}
  if (!inputPath) throw new Error('缺少输入视频')
  const sender = event.sender
  const emitProgress = (value, message) => {
    if (!currentVideoCanceled) sender.send('video-progress', { value, message })
  }
  currentVideoCanceled = false
  try {
    // 两种模式都走 Kotlin 后端逐帧（direct=单层变换 / frames=增益图）
    await ensureBackend()
    const vopts = { backendPort, format: (settings && settings.format) || 'hdr10', eclipsaOpts: (settings && settings.eclipsa) || {} } // format/eclipsa 由前端放进 settings
    if (mode === 'frames') {
      const result = await videoConverter.convertVideoFrames(
        inputPath, outputPath, settings || {}, vopts, emitProgress
      )
      return { success: true, outputPath: result.outputPath, info: result.info }
    }
    const result = await videoConverter.convertVideoDirect(
      inputPath, outputPath, settings || {}, vopts, emitProgress
    )
    return { success: true, outputPath: result.outputPath, info: result.info }
  } catch (err) {
    if (currentVideoCanceled) return { success: false, canceled: true, message: '已取消' }
    throw err
  } finally {
    currentVideoCanceled = false
  }
})

// 取消当前视频转换
ipcMain.handle('cancel-video', async () => {
  currentVideoCanceled = true
  videoConverter.cancelAllFFmpeg()
  return { ok: true }
})

// 提取源视频首帧 → JPEG data URL（源视频首帧预览）
ipcMain.handle('extract-video-first-frame', async (_event, inputPath) => {
  if (!inputPath) throw new Error('缺少视频')
  return videoConverter.extractFirstFrame(inputPath)
})

// 提取视频指定时间点的一帧 → JPEG data URL（拖动进度条生成该处 HDR 预览图）
ipcMain.handle('extract-video-frame-at', async (_event, inputPath, timeSeconds) => {
  if (!inputPath) throw new Error('缺少视频')
  return videoConverter.extractFrameAt(inputPath, timeSeconds)
})

// ---------- 生命周期 ----------
// 全局兜底：未捕获异常不应让整个应用崩溃（如管道写竞争等偶发错误）。
// 记录日志并继续运行；具体转换错误已由各 handler 的 promise 链捕获并返回给前端。
process.on('uncaughtException', (err) => {
  console.error('[uncaughtException]', err && err.stack ? err.stack : err)
})
process.on('unhandledRejection', (reason) => {
  console.error('[unhandledRejection]', reason && reason.stack ? reason.stack : reason)
})

// 单实例锁：重复启动 portable exe 时，第二实例直接退出，避免多个 Electron + 多个后端并存
const gotTheLock = app.requestSingleInstanceLock()
if (!gotTheLock) {
  app.quit()
} else {
  app.on('second-instance', () => {
    const win = BrowserWindow.getAllWindows()[0]
    if (win) {
      if (win.isMinimized()) win.restore()
      win.focus()
    }
  })

  app.whenReady().then(async () => {
    // 先清理历史遗留的孤儿后端（阻塞完成，避免误杀本实例随后拉起的后端）
    await sweepOrphanBackends()
    createWindow()
    // 预热 Kotlin 后端（不阻塞窗口）：让首帧 / 拖动进度条预览在用户操作时即时可用，避免冷启动等待
    ensureBackend().catch(() => { /* 预热失败不阻塞，首次操作会重试 */ })
  })

  app.on('window-all-closed', () => {
    app.quit()
  })

  app.on('activate', () => {
    if (BrowserWindow.getAllWindows().length === 0) createWindow()
  })

  app.on('will-quit', () => {
    videoConverter.cancelAllFFmpeg()
    if (backendProcess) {
      const pid = backendProcess.pid
      backendProcess = null
      backendPort = null
      backendStarting = null
      if (pid) {
        // 用 taskkill /T /F 杀整个进程树：child.kill() 在 Windows 上对 java 不可靠（可能只杀启动器壳）
        try {
          spawn('taskkill', ['/PID', String(pid), '/T', '/F'], { windowsHide: true, stdio: 'ignore' })
        } catch (e) { /* ignore */ }
      }
    }
  })
}
