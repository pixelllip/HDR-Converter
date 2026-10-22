// 验证「跟随 Windows 主题色」真的接上了（MD3 动态取色链路）。
//
// 背景：views/md3.js 的 applySystemTheme() 依赖 preload 暴露的 getSystemTheme()/onSystemTheme()。
// 这两个方法一度**完全没接**（preload 未暴露、main.js 未引入 systemPreferences/nativeTheme），
// 于是永远走硬编码兜底色 #006FC4 —— 在本机它恰好等于真实系统色，所以表面看"是对的"。
//
// 本测试覆盖两个最容易静默失效的点：
//   1) getSystemTheme 必须是**同步**的（md3.js 是 `var st = api.getSystemTheme()`，无 await）；
//   2) accentColor 必须带 '#'（原生 getAccentColor() 返回 8 位不带 '#'，而 hexToHsl 要求带 '#'）。
//
// 用法：node_modules\electron\dist\electron.exe tests\verify_system_theme.js
// 退出码：0 = 通过；1 = 失败。
const { app, BrowserWindow, systemPreferences, nativeTheme } = require('electron')
const path = require('path')
// 复用主进程**同一份**实现注册（standalone 测试脚本不会加载 main.js，
// 所以必须自己注册；这也正是把这段逻辑抽成 system_theme.js 的原因）
const systemTheme = require('../system_theme')

let failed = false
function check(ok, label, detail) {
  console.log((ok ? '✅ ' : '❌ ') + label + (detail ? ' — ' + detail : ''))
  if (!ok) failed = true
}

// 兜底超时：sendSync 若找不到监听器会阻塞渲染进程，避免测试永久挂起
const guard = setTimeout(() => {
  console.error('❌ 超时（30s）：疑似 sendSync 没有监听器而阻塞')
  app.exit(1)
}, 30000)

app.whenReady().then(async () => {
  const root = path.join(__dirname, '..')
  systemTheme.registerSystemTheme()
  const rawAccent = (() => {
    try { return systemPreferences.getAccentColor() || '' } catch (e) { return '' }
  })()
  const expectTheme = nativeTheme.shouldUseDarkColors ? 'dark' : 'light'

  const win = new BrowserWindow({
    show: false,
    webPreferences: {
      preload: path.join(root, 'preload.js'),
      contextIsolation: true,
      nodeIntegration: false,
      webSecurity: false
    }
  })

  await win.loadFile(path.join(root, 'views', 'home.html'))
  await new Promise((r) => setTimeout(r, 1200))

  const r = await win.webContents.executeJavaScript(`(() => {
    const api = window.electronAPI
    const out = {
      typeOfGet: typeof (api && api.getSystemTheme),
      typeOfOn: typeof (api && api.onSystemTheme),
      dataTheme: document.documentElement.getAttribute('data-theme'),
      appliedPrimary: document.documentElement.style.getPropertyValue('--primary').trim()
    }
    try { out.sync = api.getSystemTheme() } catch (e) { out.syncError = String(e && e.message || e) }
    // 同步性判定：如果是 Promise，则说明用成了 invoke —— md3.js 会静默拿到 undefined
    out.returnedThenable = !!(out.sync && typeof out.sync.then === 'function')
    return out
  })()`)

  check(r.typeOfGet === 'function', 'preload 暴露 getSystemTheme')
  check(r.typeOfOn === 'function', 'preload 暴露 onSystemTheme')
  check(!r.syncError, 'getSystemTheme() 可同步调用', r.syncError)

  if (!r.returnedThenable && r.sync) {
    check(r.sync.theme === expectTheme, 'theme 与 nativeTheme 一致',
      'got=' + r.sync.theme + ' expect=' + expectTheme)

    if (rawAccent) {
      const expectWithHash = rawAccent.charAt(0) === '#' ? rawAccent : '#' + rawAccent
      check(r.sync.accentColor === expectWithHash,
        'accentColor 与系统值一致且带 # 前缀',
        'got=' + r.sync.accentColor + ' expect=' + expectWithHash)
      check(r.sync.accentColor.charAt(0) === '#',
        'accentColor 以 # 开头（hexToHsl 的前置条件）')
    } else {
      console.log('ℹ️  本平台/环境未提供 accentColor，跳过取色断言')
    }
  } else {
    check(false, 'getSystemTheme() 必须同步返回（不能是 Promise）')
  }

  // 注意：三个页面的 <html> 里**硬编码了 data-theme="dark"**（image/video/home.html L2），
  // 所以「初始 data-theme 正确」在本机（系统为深色）是平凡成立的，不足以证明接线通。
  check(r.dataTheme === expectTheme, '初始 data-theme 与系统深浅色一致',
    'got=' + r.dataTheme + ' expect=' + expectTheme)
  check(!!r.appliedPrimary, '已应用动态调色板（--primary 非空）', r.appliedPrimary)

  // ---- 决定性验证：强制切换系统主题，页应当**实时**跟随 ----
  // 这条同时覆盖「取值正确」与「变化广播」两条链路，且不依赖本机恰好是深色：
  //   nativeTheme.themeSource='light' → system_theme.js 的 nativeTheme.on('updated')
  //   → broadcastSystemTheme() → 渲染端 'system-theme-changed' → md3.js applySystemTheme()
  const darkPrimary = r.appliedPrimary
  nativeTheme.themeSource = 'light'
  await new Promise((res) => setTimeout(res, 600))
  const afterLight = await win.webContents.executeJavaScript(`(() => ({
    dataTheme: document.documentElement.getAttribute('data-theme'),
    appliedPrimary: document.documentElement.style.getPropertyValue('--primary').trim()
  }))()`)
  check(afterLight.dataTheme === 'light',
    '切到浅色后页面实时跟随（推翻 HTML 里硬编码的 dark）',
    'got=' + afterLight.dataTheme)
  check(afterLight.appliedPrimary !== darkPrimary,
    '浅色调色板与深色不同（证明取色链路真的驱动了配色）',
    darkPrimary + ' → ' + afterLight.appliedPrimary)

  // 还原，避免影响后续测试/用户环境
  nativeTheme.themeSource = 'system'

  console.log(failed ? '\n❌ 系统主题接线验证失败' : '\n✅ 系统主题接线验证通过')
  clearTimeout(guard)
  app.exit(failed ? 1 : 0)
})
