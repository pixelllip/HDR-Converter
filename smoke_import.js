const { app, BrowserWindow } = require('electron')
const path = require('path')
app.whenReady().then(async () => {
  const w = new BrowserWindow({
    show: false,
    webPreferences: {
      preload: path.join(__dirname, 'preload.js'),
      contextIsolation: true,
    },
  })
  const logs = []
  w.webContents.on('console-message', (e, level, message, line, src) => {
    logs.push(`[L${level}] ${message} @${(src || '').split('/').pop() || ''}:${line}`)
  })
  await w.loadFile('views/video.html')
  await new Promise((r) => setTimeout(r, 1500))
  const r = await w.webContents.executeJavaScript(`(() => {
    const out = {};
    out.hasAPI = typeof window.electronAPI === 'object';
    if (window.electronAPI) {
      out.apiMethods = ['selectInputVideo','selectInputVideo','convertVideo','probeVideo','readHdrMeta'].filter(m => typeof window.electronAPI[m] === 'function');
    }
    // 检查关键监听是否已挂（间接：看函数是否可达——通过点击派发不弹窗，改为直接查看按钮存在
    out.btnChoose = !!document.getElementById('btnChooseVideo');
    out.btnBrowse = !!document.getElementById('btnBrowseVideoInput');
    out.sdrViewport = !!document.getElementById('videoSdrViewport');
    out.placeholder = !!document.querySelector('#videoSdrViewport .placeholder');
    // 尝试直接调用 loadVideoFile 等价路径（通过输入框触发）
    const inp = document.getElementById('videoInputPath');
    out.inputPath = inp ? inp.id : 'missing';
    return out;
  })()`)
  console.log('SMOKE: ' + JSON.stringify(r, null, 2))
  console.log('LOGS:')
  console.log(logs.join('\n'))
  app.exit(0)
})