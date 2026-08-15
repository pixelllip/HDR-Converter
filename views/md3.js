/* ============================================================
 * md3.js — HDR Converter 正式版 · Material Design 3 共享脚本
 * 由 design_previews/d-material3/md3.js 移植；供 views/home·image·video 三页共用
 * 覆盖：图标雪碧图、小工具、MD3 动态取色调色板（跟随 Windows 主题色 + 深浅色）
 *
 * 正式版接线（与预览版不同）：
 *   主进程：systemPreferences.getAccentColor() → 'RRGGBBAA'；nativeTheme.shouldUseDarkColors
 *   事件：systemPreferences.on('accent-color-changed')、nativeTheme.on('updated')
 *   preload 暴露 getSystemTheme() / onSystemTheme(cb)（若尚未暴露，则回退默认深色 + #006FC4）
 * ============================================================ */
(function () {
  'use strict'

  // ---------- 1. 图标雪碧图（线性图标，stroke = currentColor） ----------
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

  // ---------- 2. 小工具 ----------
  window.$id = function (id) { return document.getElementById(id) }
  window.setText = function (id, txt) { var el = document.getElementById(id); if (el) el.textContent = txt }
  window.bind = function (id, fn) { var el = document.getElementById(id); if (el) el.addEventListener('click', fn) }
  window.icon = function (name, cls) { return '<svg class="ic ' + (cls || '') + '"><use href="#i-' + name + '"/></svg>' }

  // ---------- 3. MD3 调色板：从主题色（Accent）生成 8 个 MD3 色调变量 ----------
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
  var currentAccent = '#006FC4'   // 兜底：本机检测到（去掉末尾 AA）；若 preload 暴露则用真实值
  function currentTheme() {
    var t = document.documentElement.getAttribute('data-theme')
    if (t) return t
    return (window.matchMedia && window.matchMedia('(prefers-color-scheme: light)').matches) ? 'light' : 'dark'
  }
  window.applyAccent = function (hex, label) {
    currentAccent = hex
    var p = paletteFromAccent(hex)[currentTheme()]
    var st = document.documentElement.style
    Object.keys(p).forEach(function (k) { st.setProperty(k, p[k]) })
  }

  // 正式版接线：若 preload 已暴露 getSystemTheme / onSystemTheme，则读取并监听；
  // 否则回退到默认深色 + 兜底 accent。preload 尚未暴露时不影响功能（默认琥珀橙系）。
  var api = window.electronAPI
  function applySystemTheme() {
    var accent = null
    var theme = null
    try {
      if (api && typeof api.getSystemTheme === 'function') {
        var st = api.getSystemTheme()
        if (st) {
          accent = st.accentColor || null
          theme = st.theme || null
        }
      }
    } catch (e) { /* 忽略 */ }
    if (accent) {
      if (accent.length === 9) accent = accent.slice(0, 7) // 'RRGGBBAA' → '#RRGGBB'
      applyAccent(accent, accent)
    } else {
      applyAccent(currentAccent, currentAccent)
    }
    if (theme) {
      if (theme === 'system') document.documentElement.removeAttribute('data-theme')
      else document.documentElement.setAttribute('data-theme', theme)
    }
  }
  applySystemTheme()
  try {
    if (api && typeof api.onSystemTheme === 'function') {
      api.onSystemTheme(function () { applySystemTheme() })
    }
  } catch (e) { /* 忽略 */ }
})()
