const { contextBridge, ipcRenderer } = require('electron')

contextBridge.exposeInMainWorld('electronAPI', {
  selectInputImage: () => ipcRenderer.invoke('select-input-image'),
  selectInputImages: () => ipcRenderer.invoke('select-input-images'),
  selectInputFolder: () => ipcRenderer.invoke('select-input-folder'),
  selectOutputFolder: () => ipcRenderer.invoke('select-output-folder'),
  selectOutputPath: (defaultPath) => ipcRenderer.invoke('select-output-path', defaultPath),
  convertImage: (payload) => ipcRenderer.invoke('convert-image', payload),
  convertPreview: (payload) => ipcRenderer.invoke('convert-preview', payload),
  batchConvertImages: (payload) => ipcRenderer.invoke('batch-convert-images', payload),
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
  }
})
