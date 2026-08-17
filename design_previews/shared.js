/* ============================================================
 * shared.js — 三个界面方向预览页共用的演示脚本
 * 纯静态：无 Electron / 后端依赖，浏览器或 Live Server 直接打开即可。
 * 覆盖：mock electronAPI、图标雪碧图、视图路由、选项卡、滑块绑定、
 *       canvas 生成 SDR/HDR 演示图、模拟转换进度、批量队列演示。
 * ============================================================ */
(function () {
  'use strict'

  // ---------- 1. mock electronAPI（浏览器打开时不崩） ----------
  if (!window.electronAPI) {
    window.electronAPI = new Proxy({}, {
      get: (_t, key) => (...args) => {
        console.warn('[demo] electronAPI.' + String(key) + ' → mock', ...args)
        if (key === 'getBackendStatus') return Promise.resolve({ message: '演示模式 · 静态预览' })
        return Promise.resolve(null)
      }
    })
  }

  // ---------- 2. 图标雪碧图（线性图标，stroke = currentColor） ----------
  var PATHS = {
    home: '<path d="M3 10.5 12 3l9 7.5"/><path d="M5 9.5V21h14V9.5"/>',
    back: '<path d="M19 12H5"/><path d="m11 18-6-6 6-6"/>',
    folder: '<path d="M3 7a2 2 0 0 1 2-2h4l2 2.5h8a2 2 0 0 1 2 2V17a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2Z"/>',
    folderPlus: '<path d="M3 7a2 2 0 0 1 2-2h4l2 2.5h8a2 2 0 0 1 2 2V17a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2Z"/><path d="M12 10.5v5M9.5 13h5"/>',
    play: '<path d="M7 5.5v13l11-6.5Z"/>',
    wand: '<path d="m12 3 1.2 3.3L16.5 7.5l-3.3 1.2L12 12l-1.2-3.3L7.5 7.5l3.3-1.2Z"/><path d="m19 14 .9 2.6 2.6.9-2.6.9L19 21l-.9-2.6-2.6-.9 2.6-.9Z"/><path d="M5 14v4M3 16h4"/>',
    x: '<path d="M6 6l12 12M18 6 6 18"/>',
    film: '<rect x="3.5" y="4.5" width="17" height="15" rx="2"/><path d="M7.5 4.5v15M16.5 4.5v15M3.5 9h4M3.5 15h4M16.5 9h4M16.5 15h4"/>',
    image: '<rect x="3.5" y="4.5" width="17" height="15" rx="2"/><circle cx="9" cy="10" r="1.6"/><path d="m4.5 18 5-5 3.5 3.5 3-3 3.5 3.5"/>',
    sun: '<circle cx="12" cy="12" r="4"/><path d="M12 2.5v2M12 19.5v2M2.5 12h2M19.5 12h2M5 5l1.4 1.4M17.6 17.6 19 19M19 5l-1.4 1.4M6.4 17.6 5 19"/>',
    cloud: '<path d="M7 18a4 4 0 0 1-.6-7.96A5.5 5.5 0 0 1 17 9.6 3.8 3.8 0 0 1 16.5 17Z"/>',
    check: '<path d="m5 12.5 4.5 4.5L19 7"/>',
    alert: '<path d="M12 3 2.5 20h19Z"/><path d="M12 9.5v4.5M12 17.2v.1"/>',
    refresh: '<path d="M20 12a8 8 0 1 1-2.34-5.66"/><path d="M20 3v4h-4"/>',
    chevronRight: '<path d="m9 6 6 6-6 6"/>'
  }
  var sprite = '<svg xmlns="http://www.w3.org/2000/svg" style="display:none">'
  Object.keys(PATHS).forEach(function (n) {
    sprite += '<symbol id="i-' + n + '" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">' + PATHS[n] + '</symbol>'
  })
  sprite += '</svg>'
  document.body.insertAdjacentHTML('afterbegin', sprite)

  // ---------- 3. 小工具 ----------
  function $(id) { return document.getElementById(id) }
  function setText(id, txt) { var el = $(id); if (el) el.textContent = txt }
  function bind(id, fn) { var el = $(id); if (el) el.addEventListener('click', fn) }

  // ---------- 4. 演示图生成（canvas，SDR 压暗 / HDR 提亮提饱和） ----------
  function demoImageDataUrl(hdr) {
    var c = document.createElement('canvas')
    c.width = 640; c.height = 360
    var ctx = c.getContext('2d')
    var sky = ctx.createLinearGradient(0, 0, 0, 360)
    sky.addColorStop(0, '#3d6ea5'); sky.addColorStop(0.5, '#d99a5b'); sky.addColorStop(1, '#4a3b2c')
    ctx.fillStyle = sky; ctx.fillRect(0, 0, 640, 360)
    ctx.fillStyle = '#fff2d0'; ctx.beginPath(); ctx.arc(470, 100, 44, 0, Math.PI * 2); ctx.fill()
    ctx.fillStyle = '#33513f'; ctx.beginPath(); ctx.moveTo(0, 330); ctx.lineTo(140, 205); ctx.lineTo(280, 330); ctx.closePath(); ctx.fill()
    ctx.fillStyle = '#27402f'; ctx.beginPath(); ctx.moveTo(170, 330); ctx.lineTo(360, 175); ctx.lineTo(560, 330); ctx.closePath(); ctx.fill()
    ctx.fillStyle = '#1d3023'; ctx.beginPath(); ctx.moveTo(300, 360); ctx.lineTo(500, 235); ctx.lineTo(640, 360); ctx.closePath(); ctx.fill()
    if (!hdr) { ctx.fillStyle = 'rgba(0,0,0,0.24)'; ctx.fillRect(0, 0, 640, 360) }
    var c2 = document.createElement('canvas'); c2.width = 640; c2.height = 360
    var x = c2.getContext('2d')
    x.filter = hdr ? 'brightness(1.26) saturate(1.35) contrast(1.05)' : 'none'
    x.drawImage(c, 0, 0)
    return c2.toDataURL('image/jpeg', 0.85)
  }

  function showIn(viewport, dataUrl, info) {
    if (!viewport) return
    var img = new Image()
    img.onload = function () { viewport.innerHTML = ''; viewport.appendChild(img) }
    img.src = dataUrl
    if (info) setText(viewport.id.replace('Viewport', 'Info'), info)
  }

  // ---------- 5. 视图路由 ----------
  var viewHome = $('viewHome')
  var viewImage = $('viewImage')
  var viewVideo = $('viewVideo')
  function showView(name) {
    if (viewHome) viewHome.style.display = name === 'viewHome' ? '' : 'none'
    if (viewImage) viewImage.style.display = name === 'viewImage' ? '' : 'none'
    if (viewVideo) viewVideo.style.display = name === 'viewVideo' ? '' : 'none'
  }

  // ---------- 6. 演示数据 ----------
  var DEMO_IMG = 'D:\\示例\\sunset_photo.jpg'
  var DEMO_IMG_OUT = 'D:\\示例\\sunset_photo_hdr.jpg'
  var DEMO_VIDEO = 'D:\\示例\\clip_sdr.mp4'
  var DEMO_VIDEO_OUT = 'D:\\示例\\clip_sdr_hdr.mp4'

  function setBackendReady() {
    ;['backendMethod', 'backendMethodVideo'].forEach(function (id) {
      var el = $(id); if (!el) return
      el.className = 'backend-method ready'
      var t = el.querySelector('span:last-child'); if (t) t.textContent = '演示模式 · 静态预览'
    })
  }
  setBackendReady()

  function loadDemoImage() {
    setText('inputPath', DEMO_IMG)
    setText('outputPath', DEMO_IMG_OUT)
    showIn($('sdrViewport'), demoImageDataUrl(false), '640 × 360 · 原图')
    showIn($('hdrViewport'), demoImageDataUrl(true), '640 × 360 · HDR 预览')
    var c = $('btnConvert'); if (c) c.disabled = false
    setText('status', '演示素材已载入，可直接点击"开始转换"体验进度效果。')
    var sb = $('statusBar'); if (sb) { sb.textContent = '就绪'; sb.style.color = '' }
  }

  function loadDemoVideo() {
    setText('videoInputPath', DEMO_VIDEO)
    setText('videoOutputPath', DEMO_VIDEO_OUT)
    showIn($('videoSdrViewport'), demoImageDataUrl(false), '1920 × 1080 · 29.97 fps')
    showIn($('videoHdrViewport'), demoImageDataUrl(true), '1920 × 1080 · HDR 预览')
    var c = $('btnConvertVideo'); if (c) c.disabled = false
    setText('videoStatus', '演示素材已载入，可直接点击"开始转换"。')
    var sb = $('statusBarVideo'); if (sb) { sb.textContent = '就绪'; sb.style.color = '' }
  }

  // ---------- 7. 首页 ----------
  var homeDrop = $('homeDrop')
  if (homeDrop) {
    // 卡片内的按钮（选择图片/选择视频）有自己的路由，点击不能让冒泡覆盖
    homeDrop.addEventListener('click', function (e) {
      if (e.target.closest('button')) return
      showView('viewImage'); loadDemoImage()
    })
    homeDrop.addEventListener('dragover', function (e) { e.preventDefault(); homeDrop.classList.add('drag-over') })
    homeDrop.addEventListener('dragleave', function () { homeDrop.classList.remove('drag-over') })
    homeDrop.addEventListener('drop', function (e) { e.preventDefault(); homeDrop.classList.remove('drag-over'); showView('viewImage'); loadDemoImage() })
  }
  bind('btnHomeImage', function () { showView('viewImage'); loadDemoImage() })
  bind('btnHomeVideo', function () { showView('viewVideo'); loadDemoVideo() })
  bind('btnBackHomeImage', function () { showView('viewHome') })
  bind('btnBackHomeVideo', function () { showView('viewHome') })

  // ---------- 8. 输入 / 浏览 ----------
  bind('btnChooseInput', loadDemoImage)
  bind('btnBrowseInput', loadDemoImage)
  bind('btnBrowseOutput', function () { setText('outputPath', DEMO_IMG_OUT.replace('sunset_photo', 'sunset_photo_my')) })
  bind('btnChooseVideo', loadDemoVideo)
  bind('btnBrowseVideoInput', loadDemoVideo)
  bind('btnBrowseVideoOutput', function () { setText('videoOutputPath', DEMO_VIDEO_OUT) })

  // ---------- 9. 批量队列（演示） ----------
  bind('btnImportFolder', function () {
    var names = ['IMG_0001.jpg', 'IMG_0002.jpg', 'IMG_0003.png', 'IMG_0004.jpg', 'IMG_0005.png']
    var list = $('batchList'); if (!list) return
    list.innerHTML = ''
    names.forEach(function (n, i) {
      var row = document.createElement('div')
      row.className = 'batch-item queued'
      row.innerHTML = '<span class="b-name">' + (i + 1) + '. ' + n + '</span>' +
        '<span class="b-status">排队中</span>' +
        '<button class="b-remove" title="移除"><svg class="ic" style="width:13px;height:13px"><use href="#i-x"/></svg></button>'
      row.querySelector('.b-remove').addEventListener('click', function () { row.remove() })
      list.appendChild(row)
    })
    setText('batchCount', '（5 个）')
    setText('status', '已从文件夹导入 5 张图片到批量队列。')
  })
  bind('btnBatchClear', function () {
    var list = $('batchList'); if (list) list.innerHTML = '<div class="batch-item queued"><span class="b-name">队列为空</span><span class="b-status"></span></div>'
    setText('batchCount', '')
  })

  // ---------- 10. 选项卡 ----------
  document.querySelectorAll('.tab-btn').forEach(function (b) {
    b.addEventListener('click', function () {
      document.querySelectorAll('.tab-btn').forEach(function (x) { x.classList.toggle('active', x === b) })
      document.querySelectorAll('.tab-panel').forEach(function (p) { p.style.display = p.id === b.dataset.tab ? '' : 'none' })
    })
  })

  // ---------- 11. 滑块通用绑定 ----------
  document.querySelectorAll('input[type="range"]').forEach(function (el) {
    var out = $(el.id + 'Value')
    if (!out) return
    var fmt = function () {
      if (el.id === 'hdrIntensity' || el.id === 'videoHdrIntensity') out.textContent = el.value + ' 尼特'
      else if (el.id === 'jpgQuality') out.textContent = el.value + '%'
      else if (el.id === 'videoCrf') out.textContent = el.value
      else out.textContent = parseFloat(el.value).toFixed(2)
    }
    el.addEventListener('input', fmt)
    fmt()
  })

  // ---------- 12. 自动估算（演示） ----------
  bind('btnAutoIntensity', function () {
    var s = $('hdrIntensity'); if (!s) return
    s.value = '820'
    var out = $('hdrIntensityValue'); if (out) out.textContent = '820 尼特'
    setText('status', '已自动估算峰值亮度 820 尼特（演示）。')
  })

  // ---------- 13. 模拟转换进度 ----------
  function mockProgress(btnId, fillId, labelId, statusId, done) {
    var fill = $(fillId); if (!fill) return
    var wrap = fill.closest('.progress-wrap')
    if (wrap) wrap.style.display = 'flex'
    var btn = $(btnId)
    if (btn) btn.disabled = true
    var p = 0
    var timer = setInterval(function () {
      p += 3 + Math.random() * 7
      if (p >= 100) {
        p = 100; clearInterval(timer)
        fill.style.width = '100%'
        var lb = $(labelId); if (lb) lb.textContent = '100%'
        setText(statusId, '✅ 转换完成（演示）')
        if (btn) btn.disabled = false
        if (done) done()
        setTimeout(function () { if (wrap) wrap.style.display = 'none' }, 1800)
      } else {
        fill.style.width = p + '%'
        var lb = $(labelId); if (lb) lb.textContent = Math.round(p) + '%'
        setText(statusId, '正在转换… ' + Math.round(p) + '%')
      }
    }, 60)
  }

  bind('btnConvert', function () {
    document.querySelectorAll('.tab-btn').forEach(function (b) { b.classList.toggle('active', b.dataset.tab === 'tabQueue') })
    document.querySelectorAll('.tab-panel').forEach(function (p) { p.style.display = p.id === 'tabQueue' ? '' : 'none' })
    mockProgress('btnConvert', 'progressFill', 'progressLabel', 'status', function () {
      showIn($('hdrViewport'), demoImageDataUrl(true), '640 × 360 · HDR 预览')
    })
  })

  bind('btnConvertVideo', function () {
    mockProgress('btnConvertVideo', 'videoProgressFill', 'videoProgressLabel', 'videoStatus', function () {
      showIn($('videoHdrViewport'), demoImageDataUrl(true), '1920 × 1080 · HDR 预览')
    })
  })
})()
