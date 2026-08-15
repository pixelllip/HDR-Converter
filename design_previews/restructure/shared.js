/* ============================================================
 * shared.js — 界面重构预览页共用脚本（纯静态演示）
 * 覆盖：mock electronAPI、图标雪碧图、canvas 生成演示图、
 *       通用滑块绑定、小工具函数。
 * 页面自身的交互逻辑写在各自 HTML 的内联 <script> 里。
 * ============================================================ */
(function () {
  'use strict'

  // ---------- mock electronAPI ----------
  if (!window.electronAPI) {
    window.electronAPI = new Proxy({}, {
      get: (_t, key) => (...args) => {
        console.warn('[demo] electronAPI.' + String(key) + ' → mock', ...args)
        if (key === 'getBackendStatus') return Promise.resolve({ message: '演示模式 · 静态预览' })
        return Promise.resolve(null)
      }
    })
  }

  // ---------- 图标雪碧图 ----------
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
    plus: '<path d="M12 5v14M5 12h14"/>',
    zoomIn: '<circle cx="11" cy="11" r="7"/><path d="m20 20-3.5-3.5M11 8v6M8 11h6"/>',
    fit: '<path d="M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5"/>',
    grid: '<rect x="4" y="4" width="7" height="7" rx="1"/><rect x="13" y="4" width="7" height="7" rx="1"/><rect x="4" y="13" width="7" height="7" rx="1"/><rect x="13" y="13" width="7" height="7" rx="1"/>',
    list: '<path d="M8 6h12M8 12h12M8 18h12"/><circle cx="4.5" cy="6" r="0.7"/><circle cx="4.5" cy="12" r="0.7"/><circle cx="4.5" cy="18" r="0.7"/>',
    chevronRight: '<path d="m9 5 7 7-7 7"/>',
    chevronDown: '<path d="m5 9 7 7 7-7"/>',
    layers: '<path d="m12 3 9 5-9 5-9-5Z"/><path d="m3 13 9 5 9-5"/>',
    gear: '<circle cx="12" cy="12" r="3"/><path d="M12 2.8v3M12 18.2v3M2.8 12h3M18.2 12h3M5.5 5.5l2.1 2.1M16.4 16.4l2.1 2.1M18.5 5.5l-2.1 2.1M7.6 16.4l-2.1 2.1"/>',
    trash: '<path d="M4 7h16M9 7V4h6v3M6.5 7l1 13h9l1-13"/><path d="M10 11v5M14 11v5"/>',
    external: '<path d="M14 4h6v6M20 4 11 13"/><path d="M18 13v6a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1h6"/>',
    info: '<circle cx="12" cy="12" r="9"/><path d="M12 11v5M12 8v.1"/>',
    split: '<rect x="3.5" y="4.5" width="8" height="15" rx="1.5"/><rect x="12.5" y="4.5" width="8" height="15" rx="1.5"/><path d="M12.5 4.5v15"/>'
  }
  var sprite = '<svg xmlns="http://www.w3.org/2000/svg" style="display:none">'
  Object.keys(PATHS).forEach(function (n) {
    sprite += '<symbol id="i-' + n + '" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">' + PATHS[n] + '</symbol>'
  })
  sprite += '</svg>'
  document.body.insertAdjacentHTML('afterbegin', sprite)

  // ---------- 工具 ----------
  window.$id = function (id) { return document.getElementById(id) }
  window.setText = function (id, txt) { var el = document.getElementById(id); if (el) el.textContent = txt }
  window.bind = function (id, fn) { var el = document.getElementById(id); if (el) el.addEventListener('click', fn) }
  window.icon = function (name, cls) { return '<svg class="ic ' + (cls || '') + '"><use href="#i-' + name + '"/></svg>' }

  // ---------- 演示图（SDR 压暗 / HDR 提亮提饱和） ----------
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

  // ---------- 缩略图（不同配色的变体，用于图库/队列） ----------
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

  // ---------- 通用滑块绑定（id + 'Value' 为输出节点） ----------
  document.querySelectorAll('input[type="range"]').forEach(function (el) {
    var out = document.getElementById(el.id + 'Value')
    if (!out) return
    var fmt = function () {
      var v = el.value
      if (/Peak|peak|Intensity|intensity/.test(el.id)) out.textContent = v + ' 尼特'
      else if (/Quality|quality/.test(el.id)) out.textContent = v + '%'
      else if (/Crf|crf/.test(el.id)) out.textContent = v
      else out.textContent = parseFloat(v).toFixed(2)
    }
    el.addEventListener('input', fmt)
    fmt()
  })
})()
