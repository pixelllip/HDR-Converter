/**
 * 渲染进程全局变量声明（浏览器侧）
 * ============================================================================
 *
 * 这些全局量由 `views/` 下的经典（非 module）脚本挂到 `window` 上，供三个页面的
 * 内联 `<script>` 使用：
 *   - `views/md3.js`（MD3 主题 + 小工具；9 个全局，见下）
 *   - `views/hdr_preview_media.js`（HDR 实时预览；挂 `window.HdrPreviewMedia`）
 *   - `preload.js`（contextBridge 暴露 `window.electronAPI`）
 *
 * 【重要】typeScript **无法检查 HTML 内联 <script>**，而本项目的渲染逻辑大部分写在
 * `views/image.html`（内联约 1200 行）与 `views/video.html`（内联约 1260 行）里。
 * 因此本文件的声明在阶段 0 只对 `views/*.js` 生效；等内联脚本抽成 `.ts`（阶段 4）
 * 后才会覆盖到页面逻辑本身。声明本身先行落地，是为了抽离时有现成的类型可用。
 */

interface Window {
  /** preload.js 通过 contextBridge 暴露；契约见 types/ipc.d.ts */
  electronAPI: HdrIpc.ElectronAPI

  /** views/hdr_preview_media.js L15 的 IIFE 返回值 */
  HdrPreviewMedia: HdrIpc.HdrPreviewMediaApi

  // ---------- views/md3.js 暴露的小工具（L42-62、L138） ----------
  /** getElementById 的简写 */
  $id: (id: string) => HTMLElement | null
  /** 设置元素 textContent（元素不存在时静默跳过） */
  setText: (id: string, txt: string) => void
  /** 绑定 click（元素不存在时静默跳过） */
  bind: (id: string, fn: (event: MouseEvent) => void) => void
  /** 生成雪碧图图标 <svg> 字符串，name 对应 #i-<name> */
  icon: (name: string, cls?: string) => string
  /** 按 step 格式化滑块显示值：step<1 → 两位小数，否则整数；无效值返回 '' */
  rangeValueText: (el: HTMLInputElement) => string
  /** 把值钳制到滑块 [min, max]；无效输入返回 null（所有调用点都传 input.value，即 string） */
  clampRangeValue: (el: HTMLInputElement, val: string) => string | null
  /** 应用一套以 hex 为种子色的 MD3 调色板（按当前主题写入 CSS 变量） */
  applyAccent: (hex: string, label?: string) => void
}

// ---------------------------------------------------------------------------
//  裸标识符声明
// ---------------------------------------------------------------------------
// `views/md3.js` 用 `window.applyAccent = function …` 定义，但**内部又裸调用** `applyAccent(...)`
// （浏览器里 `window.X` 与裸 `X` 等价）。TS 把「window 属性」和「全局标识符」当作两件事，
// 所以只声明 Window 接口会在 Stage 1 打开 checkJs 时报 TS2304 Cannot find name 'applyAccent'。
// 这里把两边都声明，两个命名空间各自独立、互不冲突。
declare function $id(id: string): HTMLElement | null
declare function setText(id: string, txt: string): void
declare function bind(id: string, fn: (event: MouseEvent) => void): void
declare function icon(name: string, cls?: string): string
declare function rangeValueText(el: HTMLInputElement): string
declare function clampRangeValue(el: HTMLInputElement, val: string): string | null
declare function applyAccent(hex: string, label?: string): void
