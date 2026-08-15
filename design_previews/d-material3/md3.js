/* ============================================================
 * md3.js — 方向 D · Material Design 3 全功能排版（共享脚本）
 * 从 d-material3-full.html / shared.js 拆出，供 home / image / video 三页共用
 * 纯静态：无 Electron / 后端依赖，浏览器或 Live Server 直接打开即可。
 * 覆盖：mock electronAPI、图标雪碧图、演示图生成、MD3 动态取色调色板、
 *       通用滑块绑定、小工具函数。
 * 各页面自身的交互逻辑写在各自 HTML 的内联 <script> 里。
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
  window.$id = function (id) { return document.getElementById(id) }
  window.setText = function (id, txt) { var el = document.getElementById(id); if (el) el.textContent = txt }
  window.bind = function (id, fn) { var el = document.getElementById(id); if (el) el.addEventListener('click', fn) }
  window.icon = function (name, cls) { return '<svg class="ic ' + (cls || '') + '"><use href="#i-' + name + '"/></svg>' }

  // ---------- 4. 演示图生成（canvas，SDR 压暗 / HDR 提亮提饱和） ----------
  window.demoImageDataUrl = function (hdr) {
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

  // 缩略图（不同配色变体，供批量队列/列表）
  window.demoThumbDataUrl = function (seed) {
    var c = document.createElement('canvas')
    c.width = 160; c.height = 90
    var ctx = c.getContext('2d')
    var skies = [
      ['#3d6ea5', '#d99a5b'], ['#2f5d8a', '#c98a4e'], ['#4a6fb5', '#e8b36a'],
      ['#355f7d', '#d88a5a'], ['#274d74', '#cf9a55'], ['#5a6ea8', '#e0a05e'],
      ['#2a5670', '#cc8a50'], ['#46638f', '#d99a55']
    ]
    var sky = skies[seed % skies.length]
    var g = ctx.createLinearGradient(0, 0, 0, 90)
    g.addColorStop(0, sky[0]); g.addColorStop(1, sky[1])
    ctx.fillStyle = g; ctx.fillRect(0, 0, 160, 90)
    ctx.fillStyle = '#fff2d0'
    ctx.beginPath(); ctx.arc(120 - (seed % 3) * 14, 26 + (seed % 2) * 8, 11, 0, Math.PI * 2); ctx.fill()
    ctx.fillStyle = 'rgba(40,60,40,0.9)'
    ctx.beginPath(); ctx.moveTo(0, 80); ctx.lineTo(40 + (seed % 30), 55); ctx.lineTo(85, 80); ctx.closePath(); ctx.fill()
    ctx.fillStyle = 'rgba(25,40,28,0.9)'
    ctx.beginPath(); ctx.moveTo(60, 90); ctx.lineTo(110, 58 + (seed % 5)); ctx.lineTo(160, 90); ctx.closePath(); ctx.fill()
    return c.toDataURL('image/jpeg', 0.8)
  }

  // 展示到视口
  window.showIn = function (viewport, dataUrl, info) {
    if (!viewport) return
    var img = new Image()
    img.onload = function () { viewport.innerHTML = ''; viewport.appendChild(img) }
    img.src = dataUrl
    if (info && info.id) setText(info.id, info.text)
  }

  // ---------- 5. 通用滑块绑定（id + 'Value' 为输出节点） ----------
  document.querySelectorAll('input[type="range"]').forEach(function (el) {
    var out = document.getElementById(el.id + 'Value')
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

  // ---------- 6. MD3 调色板：从任意主题色（Accent）生成 8 个 MD3 色调变量 ----------
  // 静态预览无法访问系统 API，这里用本机检测到的 #006FC4 作为默认模拟值，
  // 并列出 Windows 设置里的常见主题色供对比（点选即切换，仅演示效果）。
  function hexToHsl(hex) {
    var r = parseInt(hex.slice(1, 3), 16) / 255
    var g = parseInt(hex.slice(3, 5), 16) / 255
    var b = parseInt(hex.slice(5, 7), 16) / 255
    var max = Math.max(r, g, b), min = Math.min(r, g, b)
    var h = 0, s = 0, l = (max + min) / 2
    if (max !== min) {
      var d = max - min
      s = l > 0.5 ? d / (2 - max - min) : d / (max + min)
      if (max === r) h = (g - b) / d + (g < b ? 6 : 0)
      else if (max === g) h = (b - r) / d + 2
      else h = (r - g) / d + 4
      h *= 60
    }
    return { h: h, s: s, l: l }
  }
  function hslToHex(h, s, l) {
    h = ((h % 360) + 360) % 360; s = Math.max(0, Math.min(1, s)); l = Math.max(0, Math.min(1, l))
    var c = (1 - Math.abs(2 * l - 1)) * s, x = c * (1 - Math.abs((h / 60 % 2) - 1)), m = l - c / 2
    var r = 0, g = 0, b = 0
    if (h < 60) { r = c; g = x } else if (h < 120) { r = x; g = c }
    else if (h < 180) { g = c; b = x } else if (h < 240) { g = x; b = c }
    else if (h < 300) { r = x; b = c } else { r = c; b = x }
    function to2(v) { return Math.round((v + m) * 255).toString(16).padStart(2, '0') }
    return '#' + to2(r) + to2(g) + to2(b)
  }
  function paletteFromAccent(hex) {
    var hsl = hexToHsl(hex)
    var h = hsl.h, s = Math.max(hsl.s, 0.12)
    var sSec = s * 0.4, sTer = s * 0.55, hTer = h + 25
    return {
      dark: {
        '--primary': hslToHex(h, s, 0.78),
        '--on-primary': hslToHex(h, s, 0.16),
        '--primary-container': hslToHex(h, s, 0.32),
        '--on-primary-container': hslToHex(h, s, 0.92),
        '--secondary-container': hslToHex(h, sSec, 0.30),
        '--on-secondary-container': hslToHex(h, sSec, 0.90),
        '--tertiary-container': hslToHex(hTer, sTer, 0.34),
        '--on-tertiary-container': hslToHex(hTer, sTer, 0.90)
      },
      light: {
        '--primary': hslToHex(h, s, 0.42),
        '--on-primary': '#FFFFFF',
        '--primary-container': hslToHex(h, s, 0.90),
        '--on-primary-container': hslToHex(h, s, 0.18),
        '--secondary-container': hslToHex(h, sSec, 0.90),
        '--on-secondary-container': hslToHex(h, sSec, 0.20),
        '--tertiary-container': hslToHex(hTer, sTer, 0.90),
        '--on-tertiary-container': hslToHex(hTer, sTer, 0.22)
      }
    }
  }
  var currentAccent = '#006FC4'   // 本机通过 systemPreferences.getAccentColor() 检测到（去掉末尾 AA）
  function currentTheme() {
    var t = document.documentElement.getAttribute('data-theme')
    if (t) return t
    return (window.matchMedia && window.matchMedia('(prefers-color-scheme: light)').matches) ? 'light' : 'dark'
  }
  function applyAccent(hex, label) {
    currentAccent = hex
    var p = paletteFromAccent(hex)[currentTheme()]
    var st = document.documentElement.style
    Object.keys(p).forEach(function (k) { st.setProperty(k, p[k]) })
    var lbl = document.getElementById('seedLabel')
    if (lbl) lbl.textContent = '跟随 Windows · ' + (label || hex.toUpperCase())
    document.querySelectorAll('.seed-chip').forEach(function (c) {
      c.classList.toggle('active', c.dataset.accent === hex)
    })
  }
  document.querySelectorAll('.seed-chip').forEach(function (c) {
    c.addEventListener('click', function () { applyAccent(c.dataset.accent, c.dataset.label) })
  })
  var themeBtn = document.getElementById('themeToggle')
  if (themeBtn) {
    var order = ['dark', 'light', 'system']
    var labelMap = { dark: '模式：深色', light: '模式：浅色', system: '模式：跟随系统' }
    themeBtn.addEventListener('click', function () {
      var cur = document.documentElement.getAttribute('data-theme') || 'system'
      var next = order[(order.indexOf(cur) + 1) % order.length]
      if (next === 'system') document.documentElement.removeAttribute('data-theme')
      else document.documentElement.setAttribute('data-theme', next)
      themeBtn.textContent = labelMap[next]
      applyAccent(currentAccent)
    })
  }
  applyAccent('#006FC4', '#006FC4')
})()
