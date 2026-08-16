const { contextBridge, ipcRenderer, webUtils } = require('electron')

contextBridge.exposeInMainWorld('electronAPI', {
  // Electron 33 移除了 File.path，拖拽文件须用 webUtils.getPathForFile 取真实路径
  getPathForFile: (file) => webUtils.getPathForFile(file),
  selectInputImage: () => ipcRenderer.invoke('select-input-image'),
  selectInputImages: () => ipcRenderer.invoke('select-input-images'),
  selectInputFolder: () => ipcRenderer.invoke('select-input-folder'),
  selectOutputFolder: () => ipcRenderer.invoke('select-output-folder'),
  selectOutputPath: (defaultPath) => ipcRenderer.invoke('select-output-path', defaultPath),
  convertImage: (payload) => ipcRenderer.invoke('convert-image', payload),
  cancelImage: () => ipcRenderer.invoke('cancel-image'),
  convertPreview: (payload) => ipcRenderer.invoke('convert-preview', payload),
  estimateHdrIntensity: (payload) => ipcRenderer.invoke('estimate-hdr-intensity', payload),
  getDisplayPeakLuminance: () => ipcRenderer.invoke('get-display-peak-luminance'),
  batchConvertImages: (payload) => ipcRenderer.invoke('batch-convert-images', payload),
  batchCancelImages: (payload) => ipcRenderer.invoke('batch-cancel-images', payload),
  readImagePreview: (filePath) => ipcRenderer.invoke('read-image-preview', filePath),
  getBackendStatus: () => ipcRenderer.invoke('get-backend-status'),
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
  extractVideoFirstFrame: (inputPath) => ipcRenderer.invoke('extract-video-first-frame', inputPath),
  extractVideoFrameAt: (inputPath, timeSeconds) => ipcRenderer.invoke('extract-video-frame-at', inputPath, timeSeconds),
  onVideoProgress: (callback) => {
    const subscription = (_event, value) => callback(value)
    ipcRenderer.on('video-progress', subscription)
    return () => ipcRenderer.removeListener('video-progress', subscription)
  }
})
