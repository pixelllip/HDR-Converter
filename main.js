const { app, BrowserWindow, ipcMain, dialog } = require('electron')
const path = require('path')
const fs = require('fs')
const { convertImage, convertForPreview, detectBackend } = require('./backend/hdr_converter')

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

ipcMain.handle('select-input-image', async () => {
  const { canceled, filePaths } = await dialog.showOpenDialog({
    title: '选择输入图片',
    properties: ['openFile'],
    filters: [{ name: 'Images', extensions: ['png', 'jpg', 'jpeg', 'webp', 'bmp', 'gif'] }]
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

ipcMain.handle('convert-image', async (_event, payload) => {
  const { inputPath, outputPath, settings, backendPreference } = payload || {}
  if (!inputPath) throw new Error('缺少输入图片')

  const result = await convertImage({
    inputPath,
    outputPath,
    settings,
    backendPreference,
    onProgress: (progress) => {
      if (progress && progress.message) {
        _event.sender.send('conversion-progress', progress)
      }
    }
  })

  return result
})

// 实时预览转换（缩小尺寸快速处理）
ipcMain.handle('convert-preview', async (_event, payload) => {
  const { inputPath, settings } = payload || {}
  if (!inputPath) throw new Error('缺少输入图片')
  return convertForPreview({
    inputPath,
    settings,
    onProgress: (progress) => {
      if (progress && progress.message) {
        _event.sender.send('conversion-progress', progress)
      }
    }
  })
})

app.whenReady().then(createWindow)

app.on('window-all-closed', () => {
  app.quit()
})

app.on('activate', () => {
  if (BrowserWindow.getAllWindows().length === 0) createWindow()
})
