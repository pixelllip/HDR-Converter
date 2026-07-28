const { contextBridge, ipcRenderer } = require('electron')

contextBridge.exposeInMainWorld('electronAPI', {
  selectInputImage: () => ipcRenderer.invoke('select-input-image'),
  selectOutputPath: (defaultPath) => ipcRenderer.invoke('select-output-path', defaultPath),
  convertImage: (payload) => ipcRenderer.invoke('convert-image', payload),
  convertPreview: (payload) => ipcRenderer.invoke('convert-preview', payload),
  readImagePreview: (filePath) => ipcRenderer.invoke('read-image-preview', filePath),
  onConversionProgress: (callback) => {
    const subscription = (_event, value) => callback(value)
    ipcRenderer.on('conversion-progress', subscription)
    return () => ipcRenderer.removeListener('conversion-progress', subscription)
  }
})
