// 用 Electron 离屏渲染 d-material3-full.html，验证排版（截图输出到 _shots/）
const { app, BrowserWindow } = require('electron')
const path = require('path')
const fs = require('fs')

app.disableHardwareAcceleration()
app.commandLine.appendSwitch('no-sandbox')

const outDir = __dirname
const target = path.resolve(__dirname, '..', 'design_previews', 'd-material3-full.html')
const sleep = (ms) => new Promise((r) => setTimeout(r, ms))

app.whenReady().then(async () => {
  const win = new BrowserWindow({
    width: 1600,
    height: 950,
    show: false,
    webPreferences: { offscreen: true }
  })
  await win.loadFile(target)
  await sleep(900)

  const shot = async (name) => {
    const img = await win.webContents.capturePage()
    fs.writeFileSync(path.join(outDir, name), img.toPNG())
    console.log('saved', name)
  }
  const js = (code) => win.webContents.executeJavaScript(code)

  await shot('d_home.png')

  // 图片视图：输入输出
  await js(`document.getElementById('btnHomeImage').click()`)
  await sleep(800)
  await shot('d_image_io.png')

  // 参数质量
  await js(`document.querySelector('.tab-btn[data-tab="tabQuality"]').click()`)
  await sleep(300)
  await shot('d_image_quality.png')

  // 转换队列 + 导入文件夹演示
  await js(`document.querySelector('.tab-btn[data-tab="tabQueue"]').click()`)
  await js(`document.getElementById('btnImportFolder').click()`)
  await sleep(300)
  await shot('d_image_queue.png')

  // 滑动对比
  await js(`document.querySelector('#compareMode .seg-btn[data-mode="slider"]').click()`)
  await sleep(400)
  await shot('d_image_slider.png')

  // A/B 对比
  await js(`document.querySelector('#compareMode .seg-btn[data-mode="ab"]').click()`)
  await sleep(300)
  await shot('d_image_ab.png')

  // 视频视图
  await js(`document.getElementById('btnBackHomeImage').click()`)
  await sleep(250)
  await js(`document.getElementById('btnHomeVideo').click()`)
  await sleep(800)
  await shot('d_video.png')

  // 浅色模式
  await js(`document.getElementById('themeToggle').click()`)
  await sleep(250)
  await shot('d_video_light.png')

  app.exit(0)
}).catch((e) => { console.error(e); app.exit(1) })
