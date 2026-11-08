// 验证视频预览的亮度滚降（WebGL2 主链）与导出参数一致。
//
// 背景：视频侧新增「白点滚降系数」后，预览（hdr_preview_media.js 的 GLSL ③.2）必须与
// /video-frame 帧重建同式。本测试用真实 WebGL2 上下文渲染一帧 SDR 视频，只改 rolloff，
// 期望高光端灰阶随 rolloff 单调变亮，且 HDR 预览链没有回退到 CPU。
//
// 用法：node_modules\electron\dist\electron.exe tests\verify_video_rolloff_preview.js
// 退出码：0 = 通过；1 = 失败。
const { app, BrowserWindow } = require('electron')
const path = require('path')
const fs = require('fs')
const { spawnSync } = require('child_process')
const systemTheme = require('../system_theme')

const FFMPEG = path.join(__dirname, '..', 'backend', 'ffmpeg', 'ffmpeg.exe')
const ROOT = path.join(__dirname, '..')
const TMP = path.join(__dirname, 'tmp_video_rolloff')
const FRAME_MP4 = path.join(TMP, 'white_preview.mp4')

function check(ok, label, detail) {
  console.log((ok ? '✅ ' : '❌ ') + label + (detail ? ' — ' + detail : ''))
  if (!ok) failed = true
}
let failed = false

// 生成 1 秒高调灰视频（≈70% 码值：白点会被 SDR 画布钳到 255，看不出滚降差异；
// 中调（y<0.3）不受 rolloff 影响，必须用高光端灰阶才能观察到滚降）
// 预览只取第一帧；用视频而不是图片是因为预览模块只接受 video
fs.mkdirSync(TMP, { recursive: true })
if (!fs.existsSync(FRAME_MP4)) {
  const r = spawnSync(FFMPEG, ['-y', '-v', 'error', '-f', 'lavfi', '-i',
    'color=c=0xb3b3b3:size=64x64:rate=10', '-t', '1', '-pix_fmt', 'yuv420p', FRAME_MP4],
    { windowsHide: true })
  if (r.status !== 0) { console.error('❌ 生成测试视频失败'); process.exit(1) }
}

const guard = setTimeout(() => { console.error('❌ 超时（60s）'); app.exit(1) }, 60000)

app.whenReady().then(async () => {
  // 页面 md3.js 用 sendSync('get-system-theme')：standalone 脚本不加载 main.js，必须自己注册
  systemTheme.registerSystemTheme()
  const win = new BrowserWindow({
    show: false,
    width: 900,
    height: 700,
    webPreferences: {
      preload: path.join(ROOT, 'preload.js'),
      contextIsolation: true,
      nodeIntegration: false,
      webSecurity: false,
    },
  })
  await win.loadFile(path.join(ROOT, 'views', 'video.html'))
  // 页面自身的脚本（后端探测/主题）跑完
  await new Promise((r) => setTimeout(r, 1500))

  const slider = await win.webContents.executeJavaScript(`(() => {
    const el = document.getElementById('videoRolloff')
    return el ? { value: el.value, min: el.min, max: el.max, step: el.step } : null
  })()`)
  check(!!slider && slider.value === '0.45' && slider.min === '0' && slider.max === '1',
    '视频页存在「白点滚降系数」滑块', JSON.stringify(slider))

  const src = 'file:///' + FRAME_MP4.replace(/\\/g, '/').replace(/^\//, '')
  const init = await win.webContents.executeJavaScript(`(async () => {
    const cv = document.createElement('canvas')
    cv.id = 'hdrRenderCv'
    cv.width = 64
    cv.height = 64
    document.body.appendChild(cv)
    const badge = document.createElement('span')
    document.body.appendChild(badge)
    window.__rolloffPreview = { cv, badge }
    window.HdrPreviewMedia.init({ renderCanvas: cv, badgeEl: badge })
    window.HdrPreviewMedia.loadMedia(${JSON.stringify(src)})
    return new Promise((resolve) => setTimeout(() => resolve({
      hasMedia: !!window.HdrPreviewMedia,
      badge: badge.textContent,
    }), 1200))
  })()`)
  check(!!init.hasMedia, '预览模块已加载')
  check(!/CPU/.test(init.badge), 'HDR 预览走 WebGL2（未回退 CPU）', init.badge)

  // 读取 GL 画布中心像素（sRGB 编码码值 0..1）
  const readPixel = async (rolloff) => {
    const v = await win.webContents.executeJavaScript(`(async () => {
      window.HdrPreviewMedia.applyContent({
        tf: 'srgb', gamut: '709', dispPeak: 500, outTf: null,
        sdrGain: 500 / 203, whiteNits: 203, peakNits: 500,
        rolloff: ${rolloff}, isHdr: false
      })
      await new Promise((r) => setTimeout(r, 450))
      window.HdrPreviewMedia.refreshFrame()
      await new Promise((r) => setTimeout(r, 250))
      const { cv } = window.__rolloffPreview
      const gl = cv.getContext('webgl2')
      const px = new Uint8Array(4)
      gl.readPixels((cv.width / 2) | 0, (cv.height / 2) | 0, 1, 1, gl.RGBA, gl.UNSIGNED_BYTE, px)
      return [px[0], px[1], px[2]]
    })()`)
    return v
  }

  const r0 = await readPixel(0)
  const r1 = await readPixel(1)
  const lum = (p) => 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]
  console.log(`rolloff=0 高光灰 RGB=${r0} lum=${lum(r0).toFixed(1)}`)
  console.log(`rolloff=1 高光灰 RGB=${r1} lum=${lum(r1).toFixed(1)}`)
  check(lum(r1) > lum(r0) + 5, 'rolloff 越大预览高光越亮（预览与导出同参数生效）')

  clearTimeout(guard)
  app.exit(failed ? 1 : 0)
}).catch((e) => {
  console.error('❌', e)
  clearTimeout(guard)
  app.exit(1)
})
