/**
 * 系统主题（MD3 动态取色）—— 主进程模块
 *
 * 供 `views/md3.js` 的 `applySystemTheme()` 取色用；由 `main.js` 在创建窗口前注册。
 * 独立成模块的原因：这样 `tests/verify_system_theme.js` 能注册**同一份**实现来验证，
 * 而 standalone 的 Electron 测试脚本不会加载 main.js（否则 sendSync 找不到监听器而挂起）。
 *
 * 【两个必须保持的契约细节（都是踩过的坑）】
 *
 * 1) `getSystemTheme` 必须是**同步**通道（ipcMain.on + event.returnValue）。
 *    `views/md3.js` L153 写的是 `var st = api.getSystemTheme()` —— **没有 await**。
 *    若改成 ipcMain.handle（渲染端 ipcRenderer.invoke 返回 Promise），
 *    `st.accentColor` 会是 undefined，于是静默回退到硬编码色：接上了但没生效。
 *
 * 2) `accentColor` 必须是 **'#RRGGBBAA'（9 字符，带 `#`）**。
 *    `systemPreferences.getAccentColor()` 原生返回 8 字符 `'RRGGBBAA'`（**不带 `#`**），
 *    这里负责补 `#`。漏补的后果：md3.js 判断 `accent.length === 9` 才裁 alpha，
 *    8 字符会跳过裁剪，把 `006fc4e3` 喂给按 `slice(1,3)/slice(3,5)/slice(5,7)` 取位的
 *    `hexToHsl()` → 解析出错误的 HSL → 整套调色板偏色。
 */
const { BrowserWindow, ipcMain, nativeTheme, systemPreferences } = require('electron')

/** 读取当前系统主题快照（`HdrIpc.SystemTheme`） */
function readSystemTheme() {
  let accentColor = ''
  try {
    if (typeof systemPreferences.getAccentColor === 'function') {
      const raw = systemPreferences.getAccentColor() || '' // 'RRGGBBAA'（无 #）
      if (raw) accentColor = raw.charAt(0) === '#' ? raw : '#' + raw
    }
  } catch (e) { /* 非 Windows / 平台不支持 → 空串，渲染端回退默认色 */ }
  return {
    accentColor,
    // 用 nativeTheme 的解析结果（受 themeSource 影响），而非 CSS prefers-color-scheme
    theme: nativeTheme.shouldUseDarkColors ? 'dark' : 'light'
  }
}

/** 主题变化广播：所有窗口重跑 applySystemTheme（md3.js onSystemTheme 回调里重新取色） */
function broadcastSystemTheme() {
  const payload = readSystemTheme()
  for (const win of BrowserWindow.getAllWindows()) {
    if (!win.isDestroyed()) win.webContents.send('system-theme-changed', payload)
  }
}

/** 注册同步取值通道与主题变化监听。必须在创建窗口前调用一次。 */
function registerSystemTheme() {
  ipcMain.on('get-system-theme', (event) => {
    event.returnValue = readSystemTheme()
  })
  // accent-color-changed 是 Windows 专有事件（平台不支持时注册会抛）
  try {
    systemPreferences.on('accent-color-changed', () => broadcastSystemTheme())
  } catch (e) { /* 平台不支持，忽略（渲染端回退静态色） */ }
  nativeTheme.on('updated', () => broadcastSystemTheme())
}

module.exports = {
  readSystemTheme,
  broadcastSystemTheme,
  registerSystemTheme
}
