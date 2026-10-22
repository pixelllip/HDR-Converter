// @ts-check
// ↑ 本文件是 IPC 契约（types/ipc.d.ts）的实现方，显式开启类型检查：
//   下方 electronAPI 的每个方法都会与 HdrIpc.ElectronAPI 逐一对齐，少写/多写/签名不符即报错。
//   其余 .js 文件暂未开启（项目级 checkJs: false，采用逐文件 opt-in，见 tsconfig.json）。
const { contextBridge, ipcRenderer, webUtils } = require('electron')

/**
 * 暴露给渲染进程的 API 面。
 * @type {HdrIpc.ElectronAPI} —— 契约定义在 types/ipc.d.ts；这里的实现会被 `tsc --noEmit`
 * 逐方法校验（少写/多写/签名不符都会报错）。改这个对象时同步改 types/ipc.d.ts。
 */
const electronAPI = {
  // Electron 33 移除了 File.path，拖拽文件须用 webUtils.getPathForFile 取真实路径
  getPathForFile: (file) => webUtils.getPathForFile(file),
  selectInputImage: () => ipcRenderer.invoke('select-input-image'),
  selectInputImages: () => ipcRenderer.invoke('select-input-images'),
  selectInputFile: () => ipcRenderer.invoke('select-input-file'),
  openOutputFolder: (filePath) => ipcRenderer.invoke('open-output-folder', filePath),
  selectInputFolder: () => ipcRenderer.invoke('select-input-folder'),
  selectOutputFolder: () => ipcRenderer.invoke('select-output-folder'),
  selectOutputPath: (defaultPath) => ipcRenderer.invoke('select-output-path', defaultPath),
  convertImage: (payload) => ipcRenderer.invoke('convert-image', payload),
  cancelImage: () => ipcRenderer.invoke('cancel-image'),
  convertPreview: (payload) => ipcRenderer.invoke('convert-preview', payload),
  convertPreviewData: (payload) => ipcRenderer.invoke('convert-preview-data', payload),
  batchConvertImages: (payload) => ipcRenderer.invoke('batch-convert-images', payload),
  batchCancelImages: (payload) => ipcRenderer.invoke('batch-cancel-images', payload),
  readImagePreview: (filePath) => ipcRenderer.invoke('read-image-preview', filePath),
  getBackendStatus: () => ipcRenderer.invoke('get-backend-status'),
  // ---------- 系统主题（MD3 动态取色，供 views/md3.js 使用） ----------
  // 注意：getSystemTheme 是**同步**接口（sendSync）。md3.js L153 写的是
  // `var st = api.getSystemTheme()`（没有 await），改成 invoke（返回 Promise）会让
  // `st.accentColor` 变成 undefined，于是静默回退到硬编码色 —— 接上了但没生效。
  getSystemTheme: () => ipcRenderer.sendSync('get-system-theme'),
  onSystemTheme: (callback) => {
    const subscription = (_event, value) => callback(value)
    ipcRenderer.on('system-theme-changed', subscription)
    return () => ipcRenderer.removeListener('system-theme-changed', subscription)
  },
  onConversionProgress: (callback) => {
    const subscription = (_event, value) => callback(value)
    ipcRenderer.on('conversion-progress', subscription)
    return () => ipcRenderer.removeListener('conversion-progress', subscription)
  },
  onBatchProgress: (callback) => {
    const subscription = (_event, value) => callback(value)
    ipcRenderer.on('batch-progress', subscription)
    return () => ipcRenderer.removeListener('batch-progress', subscription)
  },
  // ---------- 视频 ----------
  selectInputVideo: () => ipcRenderer.invoke('select-input-video'),
  selectOutputVideo: (defaultPath) => ipcRenderer.invoke('select-output-video', defaultPath),
  probeVideo: (inputPath) => ipcRenderer.invoke('probe-video', inputPath),
  convertVideo: (payload) => ipcRenderer.invoke('convert-video', payload),
  cancelVideo: () => ipcRenderer.invoke('cancel-video'),
  readHdrMeta: (inputPath) => ipcRenderer.invoke('read-hdr-meta', inputPath),
  analyzeEclipsaWindows: (payload) => ipcRenderer.invoke('analyze-eclipsa-windows', payload),
  extractVideoFirstFrame: (inputPath) => ipcRenderer.invoke('extract-video-first-frame', inputPath),
  onVideoProgress: (callback) => {
    const subscription = (_event, value) => callback(value)
    ipcRenderer.on('video-progress', subscription)
    return () => ipcRenderer.removeListener('video-progress', subscription)
  }
}

contextBridge.exposeInMainWorld('electronAPI', electronAPI)
