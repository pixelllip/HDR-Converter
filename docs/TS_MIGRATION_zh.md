# TypeScript 迁移方案与进度（阶段 0 已完成）

> 本文档记录「JS → TS」的可行结论、分阶段路线，以及**阶段 0 实测发现的既有问题清单**。
> 结论：全量 TS 化技术上无障碍，但成本分布极不均匀，建议按下面的阶梯推进，不要大爆炸式重写。

## 现状盘点（实测）

| 层次 | 文件 | 行数 | 形态 |
|---|---|---|---|
| 主进程 / Node | `main.js` 921、`video_converter.js` 825、`st2094_50.js` 382、`mp4_hdr.js` 208、`st2094_50_inject.js` 181、`hevc_inject.js` 81、`preload.js` 47、`smoke_import.js` 37 | ~2682 | CommonJS，有 `module.exports` |
| 渲染层（独立文件） | `views/md3.js` 191、`views/hdr_preview_media.js` 1312 | ~1503 | IIFE 挂 `window`，经典 `<script src>` |
| 渲染层（**内联**） | `views/image.html` ≈1204、`views/video.html` ≈1259、`views/home.html` ≈59 | ~2522 | 无独立文件，写在 HTML 里 |
| 测试 | `tests/*.js` 33 个 | ~3445 | `node tests/xxx.js` |
| 构建脚本 | `build/generate-ico.js`、`build/generate-logo-png.js` | 98 | 开发用 |
| 独立预览工具 | ~~`hdr_preview/index.html` 内联 ≈2327~~ | — | **已由用户删除（2026-09，不再需要）**。注意 `views/hdr_preview_media.js` 仍在并被 `video.html` 使用，两者不是一回事 |

**不计入迁移范围**：`hdr-explorer/`（上游开源参考克隆，自带 `.git` 与独立 `tsconfig.json`，`.gitignore` 已排除）、`archive/`（已归档的 Kotlin 后端）、`design_previews/`、`experiments/`（草稿）。`backend/` 是 Rust/C++/CUDA。

## 关键技术约束

1. **渲染层不能用原生 ESM。** 页面经 `win.loadFile()` 以 `file://` 加载（`main.js:297`），Chromium 的 ESM loader 在 `file://` 下受 CORS 限制。要么上打包器（esbuild 最轻，产出经典 IIFE bundle 并保留 `window` 出口），要么注册自定义协议。
  参考：[Electron ESM 文档](https://www.electronjs.org/docs/latest/tutorial/esm)。
2. **`tsc` 无法检查 HTML 内联 `<script>`。** 而渲染逻辑主体（约 2500 行）正在里面 —— 必须先抽成 `.ts` 才谈得上类型收益。
3. **开发热重载会受影响。** `main.js:303` 的 `setupDevReload` 监听硬编码白名单 `home.html, image.html, video.html, md3.css, md3.js, preload.js`；引入编译步骤后必须把输出 bundle 加进监听名单，或用 `-w` 监听编译，否则会出现"改了 .ts 没重编、应用跑旧代码"。
4. **打包清单是白名单。** `package.json` 的 `build.files` 逐个列了 7 个 `.js`；产物路径一变必须同步改（阶段 3）。

## 阶段路线

| 阶段 | 内容 | 引入构建步骤 | 状态 |
|---|---|---|---|
| **0** | `tsconfig.json` + `types/` 契约声明 + `tsc --noEmit` 门禁 | 否 | ✅ **已完成** |
| 1 | 按下方清单逐文件加 `// @ts-check` 并修问题（先补 JSDoc，不重命名文件） | 否 | 待办 |
| 2 | 5 个 Node 二进制模块（`mp4_hdr`/`st2094_50*`/`hevc_inject`）转 `.ts`，`tsc` 输出 CJS 到 `dist/` | 是（仅 Node 侧） | 待办 |
| 3 | `main.ts` / `preload.ts`；改 `package.json` 的 `main` + `build.files` | 是 | 待办 |
| 4 | 渲染层：内联脚本抽成 `src/renderer/*.ts`，esbuild 打包到 `views/dist/`，改 `<script src>`，扩 `setupDevReload`，重写 `tests/check_inline_syntax.js` | 是（渲染侧） | 待办 |
| 5 | 33 个测试脚本迁 TS（可选，收益小） | 是 | 待办 |

**不建议动**：`hdr-explorer/`、`design_previews/`、`experiments/`、`archive/`。

## 阶段 0 交付物与用法

```
tsconfig.json            # noEmit + allowJs；checkJs:false（逐文件 opt-in）
types/ipc.d.ts           # IPC 契约单一事实来源（HdrIpc.* 全局可见）
types/globals.d.ts       # window.electronAPI / HdrPreviewMedia / md3 全局
tests/check_types.js     # 门禁：node tests/check_types.js（= npm run typecheck）
```

顺带修复（详见下方「系统主题接线」）：

```
system_theme.js                    # 新增：系统主题色/深浅色（主进程模块）
tests/verify_system_theme.js       # 新增：验证接线（含实时跟随）
```

`preload.js` 首行加了 `// @ts-check`，其 `electronAPI` 对象带 `@type {HdrIpc.ElectronAPI}`，
因此**契约是被真实校验的**——少写/多写/签名不符会立刻报错（已用"故意改名"验证：报 TS2561）。

```powershell
npm run typecheck                                             # ✅ 类型检查通过
node_modules\electron\dist\electron.exe tests\verify_system_theme.js   # ✅ 系统主题接线验证通过
```

### 为什么采用「逐文件 opt-in」而不是全局 `checkJs: true`

全局打开会一次暴露 **62 个既有问题**（下面清单），门禁当场变红、失去意义。
`checkJs: false` + 逐文件 `// @ts-check` 让门禁今天就是绿的，并且每修好一个文件就"棘轮"收紧一格。

---

## 阶段 1 待修清单（62 个错误，实测于 `tsc --noEmit --checkJs`）

按错误码统计：`TS2339`×30、`TS1005`×12、`TS2304`×8、`TS2502`×3、`TS2322`×2、`TS2353`×2、`TS2769`×2、`TS1064`×1、`TS2345`×1、`TS2552`×1。

按文件：`video_converter.js` 22、`st2094_50.js` 15、`views/md3.js` 11、`main.js` 4、`mp4_hdr.js` 4、`hevc_inject.js` 3、`st2094_50_inject.js` 2、`views/hdr_preview_media.js` 1。

### A. 既有 JSDoc 写法不合 TypeScript 语法（约 20 处，纯注释问题，改注释即可）

这些注释把「名字列表」放在类型位置，JSDoc 工具容忍，但 TS 会当类型解析并失败。

| 位置 | 写法 | 应改为 |
|---|---|---|
| `mp4_hdr.js:156` | `@param opts { maxCll, maxFall, mastering }` | `@param {{maxCll:number, maxFall:number, mastering:object}} opts` |
| `mp4_hdr.js:157` | `@returns { inserted, maxCll, maxFall }` | `@returns {{inserted:number, maxCll:number, maxFall:number}}` |
| `st2094_50.js:33` | `@param cvt { hasCustomHdrReferenceWhite, hdrReferenceWhite?, … }` | 合法对象类型 |
| `st2094_50.js:95` / `:110` | `{ncp, …}` / `{ applicationVersion, … }` | 合法对象类型 |
| `st2094_50_inject.js:103` | 同上 | 合法对象类型 |
| `video_converter.js:262` | `@returns { args: string[], name: string }` | `@returns {{args: string[], name: string}}` |
| `video_converter.js:790` / `:805` | `@returns { dataUrl, framePath }` | `@returns {Promise<{dataUrl:string, framePath:string}>}` |

**额外注意 `video_converter.js:175`**：`probeCodedHeight` 的 JSDoc 返回类型既**缺 `Promise<>`**（async 函数 → TS1064），又**漏了 `colorTransfer`/`colorPrimaries`**（函数实际返回 4 个字段，见 `:181`），并连带 `:335` 报错。这是注释与实际不符，属文档缺陷。

### B. 从空对象默认值解构，缺参数类型（TS2339 / TS2353）

运行时正常（调用方都传了实参），补 JSDoc 即可：

- `hevc_inject.js:47` / `:74` / `:76` — `{ buildSeiNal, position = 'after-aud' } = {}`
- `video_converter.js:67` — `{ onProgress, durationUs, cwd } = {}`

### C. 真实缺陷（建议独立修复，与 TS 迁移解耦）

1. ~~**`main.js:308` / `:317` — `fs.watch` 的 `interval` 选项非法。**~~
   **✅ 已修复**（见「已修复的真实缺陷」§1）。
   `fs.watch(dir, { interval: 200 }, cb)`：`interval` 属于 `fs.watchFile`，**不是 `fs.watch` 的合法选项**，Node 静默忽略 → 想设 200ms 轮询间隔的意图从未生效。删除该选项即行为等价（Node 本就忽略未知选项）。
2. **`video_converter.js:691` / `:720` / `:721` — 视频返回值的形状是漂移的。**
   `outInfo = {...info}` 之后动态追加 `colorRange` / `format` / `eclipsa`，类型上不存在。配合下面「契约缺口」第 1 条一起看，`/convert-video` 的返回值没有稳定契约。
3. **`video_converter.js:629-637` — `encoder` 被收窄成 `never`。**
   `let encoder = null` 后在闭包 `feedPam` 里赋值，TS 控制流分析在外部作用域看不到 → 收窄成 `never`。**代码运行时是对的**，属 TS 对闭包的已知限制，给变量加 JSDoc 类型即可。
4. **`views/md3.js:162` / `:164` — 裸调用 `applyAccent(...)`**，而定义是 `window.applyAccent`。已在 `types/globals.d.ts` 补了裸标识符声明。
5. **`views/md3.js:57` / `:67` / `:72` / `:74` / `:186`** — `e.target` 是 `EventTarget`，缺 `HTMLElement` 收窄（`tagName`/`type`/`select`/`blur`/`closest`）。运行时正常。
6. **`views/hdr_preview_media.js:15`** — IIFE 返回值与 `HdrIpc.HdrPreviewMediaApi` 不完全匹配（需完整错误信息定位，可能是 `getContentParams()` 的 `gamut` 可空性或 `applyContent(p.tf)` 的 `undefined`）。
7. **`main.js:873`** — `reason.stack`：`unhandledRejection` 的 reason 是 `unknown`，需收窄。

### D. 顺带发现的既有缺陷（**与本次改动无关**，均在 HEAD 版本复现）

1. **`tests/check_inline_syntax.js` 现在就是失败的。**
   `views/video.html` L487 引用 `getElementById('vSdrPqWarn')`，但**该 id 在整个文件里不存在**（HEAD 里 `id="vSdrPqWarn"` 出现 0 次）→ SDR+PQ 警告分支是死代码，测试报 `ID MISSING in views/video.html: vSdrPqWarn`。
2. ~~**`views/md3.js` 的「跟随 Windows 主题色」实际没接上。**~~
   **✅ 已修复 —— 见下节「系统主题接线」。** 原状：`views/md3.js:152`/`:173` 调用 `api.getSystemTheme()` 与 `api.onSystemTheme()`，但 **`preload.js` 从未暴露这两个方法**（`main.js` 也没引入 `systemPreferences` / `nativeTheme`）。代码有 `typeof … === 'function'` 兜底所以不报错，但效果是一直走回退分支（硬编码 `#006FC4`）。
3. **三个页面把 `data-theme="dark"` 硬编码在 `<html>` 上**（`home.html` / `image.html` / `video.html` 第 2 行）。
   配合上一条，意味着**深浅色跟随也从未生效**：即使用户 Windows 是浅色，应用也一直渲染深色。
   修复后由 `applySystemTheme()` 按 `nativeTheme` 覆写该属性（已验证：切浅色 → `data-theme` 实时变 `light`）。

---

## 系统主题接线（已修复）

新增 `system_theme.js`（主进程模块）+ `tests/verify_system_theme.js`（验证），并改了 `main.js`、`preload.js`、`views/md3.js`。

**两个会让接线「接上了但没生效」的陷阱**（均已验证）：

1. **必须同步取值。** `views/md3.js` L153 写的是 `var st = api.getSystemTheme()` —— **没有 `await`**。
   若用 `ipcRenderer.invoke`（返回 Promise），`st.accentColor` 就是 `undefined`，于是静默回退到硬编码色。故用 `ipcRenderer.sendSync` + `ipcMain.on(..., e.returnValue)`。
2. **`accentColor` 必须带 `#`。** `systemPreferences.getAccentColor()` 原生返回 **8 字符 `'RRGGBBAA'`（不带 `#`）**，
   而 `md3.js` 仅在 `length === 9` 时才 `slice(0,7)` 裁 alpha，`hexToHsl()` 又按 `slice(1,3)/slice(3,5)/slice(5,7)` 取位
   → 直接透传 8 位串会解析出错误 HSL、整套调色板偏色。由 `system_theme.js` 负责补 `#`。

另外把 `md3.js` 的 `applyAccent()` 挪到 `setAttribute('data-theme')` **之后**（原顺序会让首次加载按 `matchMedia` 猜模式取色，可能与 `nativeTheme` 不一致）。

**为什么抽成独立模块**：standalone 的 Electron 测试脚本不会加载 `main.js`，
若把逻辑写在 `main.js` 里，`tests/verify_system_theme.js` 就无法注册同一份实现
（实测表现为 `sendSync() ... without listeners` 并挂死）。抽成 `system_theme.js` 后测试注册的就是生产代码。

**注意**：`package.json` 的 `build.files` 是白名单，新增 `system_theme.js` **必须同步登记**，否则打包后缺模块。

验证：`node_modules\electron\dist\electron.exe tests\verify_system_theme.js`
覆盖：同步性、`#` 前缀、与系统值一致、以及**强制 `nativeTheme.themeSource='light'` 后页面实时跟随**
（`data-theme` dark→light，`--primary` `#8fceff` → `#0079d6`）。

---

## 已修复的真实缺陷（2026-09）

以下是阶段 0 查出来的问题中，**属于真实缺陷**（而非单纯类型噪音）的部分，已逐个修复并验证。

### 1. `fs.watch` 传了非法选项 `interval`（`main.js`）

`fs.watch(dir, { interval: 200 }, cb)` —— `interval` 是 **`fs.watchFile`** 的轮询选项；
`fs.watch` 在 Windows 走 `ReadDirectoryChangesW`，是事件驱动，**不接受** `interval`，Node 静默忽略。
即"200ms 轮询间隔"这个意图从未生效（真正的防抖靠下面 150ms 的 `reloadTimer`）。
**修复**：删掉该选项（行为等价），两处都改（`views/` 与 `preload.js` 的 watcher）。

### 2. `convert-video` 丢掉 `encoder`（`main.js`）

`video_converter.js:738` 一直返回 `{ outputPath, info, encoder }`，但 `main.js` 的 handler
只回 `{ success, outputPath, info }`，把 `encoder` 丢了。于是 `views/video.html:1249` 的
`r.encoder || currentEncoder()` **永远走 fallback** —— 界面显示的是"请求的编码器"而不是
"真正用上的"（请求 nvenc 但 NVENC 不可用会降级 x265，界面看不出来）。
**修复**：三个 `return` 分支都回传 `encoder: result.encoder`。

### 3. `#vSdrPqWarn` 元素从未存在（`views/video.html`）

JS 在 `L487` 取 `getElementById('vSdrPqWarn')`、`L1041-1044` 控制其显隐
（注释写明「SDR + PQ 警告（A1：提示不阻止）」），但**该 id 在整个 HTML 里不存在** →
提示永不显示（死代码），并让 `tests/check_inline_syntax.js` **一直在失败**。
**修复**：按同面板内既有的 `#vEclipsaOutputNote` 样式补齐该元素（`display:none` 初始，
由 JS 控制显隐），未新造视觉样式。`check_inline_syntax.js` 现已通过。

### 4. `httpJson()` 不检查 HTTP 状态码（`main.js`）

与同文件的 `httpBinary()` 不一致：`httpBinary` 对 `statusCode >= 400` 会 reject，
而 `httpJson` 直接 `JSON.parse` —— 后端 500 的 `{"error":"预览失败"}` 会被当成
**正常返回值** resolve 给渲染进程，前端只看到 undefined 字段，报出"后端未返回图像"这类误导提示。
**修复**：`>= 400` 时 reject，并优先取错误体里的 `error` 字段作为消息；非 2xx 的 422（请求体反序列化失败）也一并给出真实原因。

**实测各路由状态码**（界定影响面，非推断）：

| 路由 | 成功 | 失败 |
|---|---|---|
| `/health`、`/status`、`/progress`、`/batch/progress`、`/cancel`、`/batch/cancel` | 200 | — |
| `/convert` | 200 | **仍 200** + `{success:false}`（该路由永不 500，故不受影响） |
| `/preview` | 200 | **500** `{"error":"…"}` → 现在 reject |
| 任意 POST（请求体畸形） | — | **422** 明文 → 现在 reject（原先只是"响应解析失败"） |

前端调用点（`image.html` 的 `previewWithRetry`）本来就有 `try/catch`，故无回归。
`/estimate` 与自动估算功能已在 2026-10 移除，不再有第二个调用点。
已实测应用启动正常、后端经 `httpJson` 就绪（成功路径未被误判）。

### 5. `tests/backend_test_util.js` 的 `stopping` 未声明（顺带）

`process.on('exit')` 里 `if (proc && !proc.killed && !stopping)` —— `stopping` **从未声明**，
求值即抛 `ReferenceError` → 异常退出时 `stopBackend()` **永不执行**，遗留 java 进程。
**修复**：补上 `let stopping = false` 并真正置位。（该文件只服务已归档的 Kotlin 后端对照测试。）

---

## 阶段 0 发现的「契约缺口」（比语法问题更值得处理）

这些是**类型化真正换来的东西**——把隐式约定写出来后就露出来的裂缝：

1. ~~**`/convert-video` 丢掉了 `encoder`。**~~
   **✅ 已修复**（见「已修复的真实缺陷」§2）。
   `video_converter.js:738` 返回 `{ outputPath, info, encoder }`，但 `main.js:809/828/835` 只回 `{ success, outputPath, info }`。
   于是 `views/video.html:1249` 的 `r.encoder || currentEncoder()` **永远走 fallback** ——
   界面显示的是"请求的编码器"而不是"实际生效的编码器"（例如 nvenc 不可用降级到 x265 时看不出来）。
2. **后端 500 会被当成正常返回值。** `main.js:59-89` 的 `httpJson()` **不检查 `statusCode`**，任何响应都 resolve。
   `/preview`（500 `{"error":"预览失败"}`）会把错误体当数据回给渲染进程。
   现状：`preview` 路径只判断 `result.dataUrl`，会退化成"后端未返回图像"；而 `image.html:794` 用 `result.success !== false` 判定成功——**理论上会把错误体当成功**（`/convert` 实际永不 500，所以暂未爆）。
3. **`/cancel` 与 `/batch/cancel` 返回字符串 `{"ok":"true"}`**（`server.rs:416`/`:731`），而 `main.js:521` 的短路分支返回布尔 `{ok:true}`。同一字段两种类型（渲染进程目前忽略返回值）。
4. **`readHdrMeta` 全仓库零调用**：`preload.js:38` 暴露了它，`main.js:655` 实现了它，但 `views/` 下没有任何调用点。
5. **进度订阅从不退订**：`image.html:830`/`:993`、`video.html:1605` 都丢弃了 `on*Progress` 返回的取消函数，全仓库无 `removeListener` → 重新执行页面脚本会叠加监听器。

## 一句话建议

阶段 1 请**先只修 A 类和 B 类**（纯注释/annotations，零运行时风险），每修好一个文件就在首行加 `// @ts-check`；
C 类里的第 1 条（`fs.watch` 的 `interval`）建议立刻单独修 —— 那是真实失效的功能意图。
渲染层（阶段 4）请单独成一次提交，并**在 HDR 显示器上人工目视验证预览链**，因为它几乎没有自动化测试覆盖。
