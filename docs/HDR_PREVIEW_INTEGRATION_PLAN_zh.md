# HDR 预览台整合方案：所见即所得导出 + 同步对比 + 文档勘误

> 目标：把 `hdr_preview/index.html` 研究透，做成**所见即所得（WYSIWYG）导出**——导出的视频与预览画面逐像素一致；
> 保留并增强「同一播放控件控制两个视频进度」的同步对比；修正 README/RENDERING 中不准确的结论；
> 并给出接入本仓库 Electron 应用（`hdr_electron`）的整合路线。
>
> 本文档基于对以下文件的通读与核对（行号均以当前工作区为准）：
> - `hdr_preview/index.html`（2698 行，唯一实现）
> - `hdr_preview/README.md`、`hdr_preview/RENDERING.md`
> - 上游克隆 `hdr-explorer/app/**`（app.ts / panels / color_functions.ts / webm.ts / download.ts）
> - Electron 宿主 `views/video.html`、`video_converter.js`、`main.js`、`backend/ffmpeg/*`

---

## 0. 摘要（TL;DR）

| 事项 | 现状 | 本方案 |
| --- | --- | --- |
| 实时 HDR 预览 | 场景 CPU 链 + 媒体 WebGL2 GPU 链（CPU 回退），`state → buildState → currentState → renderAll()` 全量联动，参数改动即时生效 | 保持不变（研究的对象与基石） |
| 所见即所得导出 | ❌ 无 | 新增：**导出 = 重渲染面板所见像素**。三条实现路径：X 实时画布录制（页内、零依赖）；Y WebCodecs 加速（页内）；Z Electron + ffmpeg rawvideo（最快、可写 10bit/HDR 元数据） |
| 同步对比 | 单个 `mediaVideo` 驱动「原生 + 重渲染」两画布，播放/暂停/进度/循环共用 | 保留；新增**双源模式**（原始文件 vs 导出文件）共用同一传输控件做 A/B 回验；Electron 侧与其 `vidCmpStage` 对齐 |
| 文档勘误 | README L37/38/42 存在过时/矛盾的结论 | 按 §5 逐条修正 |
| Electron 整合 | 尚未接入（README「后续路线」） | 按 §6 P2 阶段：新视图 + IPC + ffmpeg/HDR 注入复用 |

---

## 1. 研究成果：实时 HDR 视频预览的实现原理

### 1.1 总体数据流

```
UI 控件（tf/gamut/ev/tm/dispGamut/dispTf/dispPeak/gm/ootf/showClamped…）
   │ bindUI() 事件 → state 对象（L1229）
   ▼
buildState()（L1235）→ currentState（渲染状态对象，含 4096 点 OETF/EOTF/显示 OETF LUT、
                       色域转换矩阵 conv/to2020/from2020、maxNits、EV 乘数）
   │ renderAll()（L1768）
   ├─► 链路 A（CPU）：renderPreview → chainPixel → decodeAndRenderCode      [场景 & GL 回退]
   ├─► 链路 B（GPU）：startMediaPump → renderMediaFrame → pumpMediaFrameGl  [媒体主链]
   ├─► 曲线图 / CIE 色度图 / 图例（纯 JS 绘制，共享同一数学核心）
   └─► mediaGlRefresh()：参数变化时只重刷 uniform 并重绘
```

### 1.2 媒体实时预览的两条链路（重点）

**主链（GPU，`MediaGlRenderer` L2273，模仿上游 `BaseWebgl2Renderer` + `HdrRenderer`）**

```
<video> 帧
  │ createImageBitmap(src, {colorSpaceConversion:'none'})  ← 取未做显示转换的帧（HDR 码值）
  │   （失败回退 drawImage → canvas）
  ├─► texImage2D/texSubImage2D → RGBA16F 纹理（UNPACK_COLORSPACE_CONVERSION_WEBGL=NONE）
  ▼
fragment shader（HDR_GL_FS L2213 = #version 300 es + kColorFunctionGlsl L1917）
  1. ApplyOetfInv(rgb, texture_trfn)        ← 按所选内容传递函数解码（PQ/HLG/sRGB/γ2.2…）
  2. ApplyOotfAdaptiveHlg（仅 HLG）          ← BT.2100 OOTF，γ 随显示峰值自适应（BT.2408 Note 5f）
  3. HLG ×exp2(target_log2_headroom)；PQ ×10000/203   ← 展到扩展 SDR（203=参考白）
  4. ToDisplayWithClamping：primariesConvert(内容→显示色域) × linear_scale → ApplyOetf(显示端) → clamp
  5. show_clamped：超 headroom→粉，色域转换后超→青
  ▼
drawing buffer：drawingBufferColorSpace='display-p3'
  + configureHighDynamicRange({mode:'extended'}) + drawingBufferStorage(RGBA16F)（HDR 画布探测）
```

uniform ↔ 页面控件映射（L2367–2377）：`texture_trfn=TF_TO_CICP[tf]`、`texture_primaries=GAMUT_TO_CICP[gamut]`、
`framebuffer_trfn=DISPTF_TO_CICP[dispTf]`、`framebuffer_primaries=GAMUT_TO_CICP[dispGamut]`、
`target_log2_headroom=log2(dispPeak/203)`、`presentation_display_peak_luminance=dispPeak`、`show_clamped`。

**回退链（CPU，`renderMediaFrameCpu` L2497）**：GL 初始化/帧更新失败时，
`drawImage` 取浏览器显示转换后的帧 → `getImageData` → 逐像素 `decodeAndRenderCode`
（= 链路 A 后半段：EOTF 解码 → HLG OOTF → 色域矩阵 → tonemap → 超色域处理 → 显示 OETF）。

> ⚠ 关键差异（影响导出参考链的选择，见 §3.1）：
> - CPU 链实现了 UI 的 **none / Reinhard / ACES 三种色调映射**；GLSL 链（上游语义）只有
>   headroom 缩放 + clamp，**不使用 tm 下拉**。
> - CPU 矩阵基于 XYZ（D65 白点推导）；GLSL 矩阵是上游逐字搬运的 **XYZ-D50** 常量矩阵。
> - GLSL 链的 HLG OOTF 为 γ 自适应版本；CPU 链为固定 `Y^0.2`（BT.2100）且仅 HLG 生效。
> 即：同一个参数集下，GL 面板与 CPU 面板是"同源但数值不完全相同"的实现。

### 1.3 同步对比现状（已经实现的部分）

- 单个隐藏 `<video id=mediaVideo>`（L2475）是唯一时间轴源；
- `mediaPlayBtn / mediaSeek / mediaTime / mediaLoop / mediaFrameScale`（L1864–1904）驱动它；
- 同一帧同时绘制到两张画布：`mediaNativeCv`（浏览器原生色彩管理参考）与 `mediaRenderCv`（重渲染）；
- 帧泵 `startMediaPump`（L2526）在 `currentTime` 变化时刷新两画布 → **进度天然同步，零漂移**。
- 上游 hdr-explorer 也是同一模式：单 `<video id=MyVideo>` + 时间轴驱动全部面板（app.ts `myVideoEl`），
  本页与上游同构；Electron 宿主 `views/video.html` 另有双 `<video>`（SDR/HDR）+ 单进度条的
  `vidCmpStage`（L306–323）做转换前后对比——两种"同步对比"形态都已存在。

### 1.4 元数据解析（导入时自动检测）

- 容器层：ISO-BMFF `colr(nclx)/mdcv/clli` 与样本表（`parseIsobmff` L616）、Matroska `Colour`
  （`parseWebm` L769）、AVIF meta 扫描兜底；
- 码流层：H.264/H.265 SEI（144/137）与 AV1 OBU（`parseSampleMetadata` L1082），T.35 →
  HDR10+ (2094-40) / 2094-50 AGTM 摘要（`parseT35`/`parseHdr10p`/`parseAgtmSummary`）；
- 导入成功自动把 CICP 填入传递函数/色域下拉（`loadMediaFile` L2640，同上游「自动检测」）。

### 1.5 与上游对应关系（核对结论）

- `kColorFunctionGlsl` 与上游 `color_helpers/color_functions.ts` 逐字一致（已对照 11 色域矩阵、
  9 传递函数、`ToDisplayWithClamping`）；HLG 自适应 OOTF 搬运自 `panels/hdr_renderer.ts`
  （含其 `L_W >= 400.0 || L_W <= 2000.0` 条件——上游原文即 `||`，本页为忠实搬运，见 §5-4）；
- 上游导出能力：`webm.ts`（EBML WebM 写入器）、`download.ts`（Blob/APNG 下载）、
  `app.ts downloadAgtmVideo`（AGTM 元数据重封装导出）——**本方案的 WebCodecs+WebM 导出可直接借鉴**；
- 本页与上游主要差异（README 已述且属实）：仅首样本摘要、无 `navigator.hdr`、单 HDR 渲染面板
  （非多面板/无 3D LUT、无 AGTM 曲线面板）。

---

## 2. 整合方案总览

```
┌────────────────────── hdr_preview（单文件页，本次改造主体）─────────────────────┐
│  现有：预览 4 屏 + 媒体双面板 + 同步传输 + 元数据解析                                  │
│  新增：┌ 导出面板（范围/分辨率/帧率/编码/码率/进度/取消）                              │
│        ├ 参数冻结（导出期间快照 currentState）                                        │
│        ├ 导出管线：X 实时画布录制 | Y WebCodecs 加速 | Z Electron ffmpeg              │
│        ├ 导出后自动回载 → 双源 A/B 同步对比（同一传输控件）                           │
│        └ 场景快照 PNG 导出（顺带的小功能）                                            │
└──────────────────────────────┬───────────────────────────────────────────┘
                               │ 整合（P2）
┌──────────────────────────────▼───────────────────────────────────────────┐
│  Electron 应用（hdr_electron）：新增 views/preview.html（内嵌本页）            │
│   · preload 暴露：chooseSavePath / exportStart / exportFrame / exportDone / │
│     exportProgress / exportCancel                                          │
│   · 主进程：复用 backend/ffmpeg（rawvideo stdin）、mp4_hdr.js（mdcv/clli/colr│
│     注入）、st2094_50_inject.js → 可导出 10bit HDR10/HLG 直出文件            │
└────────────────────────────────────────────────────────────────────────────┘
```

---

## 3. 所见即所得导出设计（核心）

### 3.1 语义定义

**WYSIWYG 约定**：导出视频的第 N 帧像素 == 播放到同一时刻时「重渲染面板」(`mediaRenderCv`)
的所见像素。参数集为导出开始瞬间的 `currentState` 快照（含 tf/gamut/tm/dispGamut/dispTf/
dispPeak/gm/ootf/showClamped/帧宽；媒体链路不使用 EV，与预览一致，需在 UI 注明）。

**参考链选择（要做的一个决策）**：

| 方案 | 做法 | 优点 | 缺点 |
| --- | --- | --- | --- |
| R1 画布直录（推荐默认） | `captureStream(mediaRenderCv)` + MediaRecorder | 与所见零分歧、实现最小、天然支持 HDR 画布 | 仅 1× 实时；帧率≈画布刷新率 |
| R2 GL 读回 | FBO(RGBA16F) 渲染 → readPixels → 导出画布 | 与 GL 面板逐像素一致、可加速 | 需 FBO/扩展处理，SDR/HDR 分支 |
| R3 CPU Worker 精确 | Web Worker 中跑 `decodeAndRenderCode` | 跨设备像素确定、可测试、可加速 | 与 GL 面板有 §1.2 微小数值差 |

> 推荐组合：**默认 R1 + 可选 R3**。R1 保证"所见即所得"的绝对语义（用户看到什么就导出什么）；
> R3 提供确定性（自动化验证、Electron 路径的帧源）。R2 作为 R1 在"以 >1× 速度导出"需求下
> 的演进，与 Z 路径结合使用。三者的共同点：帧源、参数、元数据一致。

### 3.2 导出选项（UI schema）

```js
{
  source: 'source' | 'exported',        // 导出对象（第一阶段仅 source）
  range:   { start: 0, end: duration }, // 时间范围（秒）
  width:   720,                          // 帧宽（默认=mediaFrameScale）
  fps:     'source' | 24 | 30 | 60,     // 帧率（source=按 requestVideoFrameCallback 实际帧）
  chain:   'canvas' | 'cpu-worker',     // R1 / R3
  codec:   'vp9' | 'av1' | 'h264',      // MediaRecorder 支持情况运行时探测，不可用则灰化
  bitrate: 0,                            // 0=自动
  keepClampedMarkers: true,              // showClamped 粉/青标记是否保留（WYSIWYG 默认保留）
  filename: 'xxx_preview_<params>.webm',
  // Electron 路径追加：
  container: 'mp4' | 'webm' | 'mkv',
  hdrDirect: false,                      // 见 3.6「HDR 直出」模式
  includeAudio: false,                   // Electron 路径可 -map 0:a 转封装（重编码，P2）
}
```

### 3.3 三条实现路径

**路径 X —— 实时画布录制（页内、零依赖，P0 先做这个）**

1. 导出开始时快照 `const snap = buildState()` 并冻结参数控件（禁用 + 遮罩提示）；
2. 复制 `mediaRenderCv` 当前尺寸状态 → 建**专用 2D 导出画布** `exportCv`（不直接录 WebGL 画布，
   规避 `preserveDrawingBuffer` 风险）；
3. 播放 `mediaVideo`，用 `requestVideoFrameCallback` 每来一帧：
   - 走原预览逻辑渲染到 `mediaRenderCv`（GL 或 CPU 回退，与所见一致）；
   - `exportCtx.drawImage(mediaRenderCv, 0, 0)` 拷入 `exportCv`；
4. `exportStream = exportCv.captureStream(fps)` → `MediaRecorder(exportStream, {mimeType, videoBitsPerSecond})`
   → `ondataavailable` 收集 chunks → `onstop` 合成 Blob 下载；
5. 期间显示进度（已渲染帧/预估总帧，取源 `duration×fps`），支持取消（`recorder.stop()` + abort）；
6. 完成后恢复参数控件。

**路径 Y —— WebCodecs 加速（页内，P1）**

1. 帧源与 X 相同但**不按墙钟走**：seek 步进 `currentTime += 1/fps` → 等 `seeked` → 渲染一帧；
   （或用 `playbackRate>1` + RVFC 快进逐帧）
2. `new VideoEncoder({output: chunk => muxer.feed(chunk), error})`（avc/vp9/av1 运行时探测）；
3. 移植上游 `webm.ts` 的 EBML/WebM writer（Apache-2.0，与仓库现有 `mp4_hdr.js` 风格一致）：
   `new WebMWriter({width,height,frameRate})` → append chunk → finish → Blob；
4. 好处：帧时间戳显式可控 → **可 >1× 速度导出**、帧率精确为所选 fps、可逐帧对账。

**路径 Z —— Electron + ffmpeg（P2，整合宿主后首选）**

1. 渲染器：按 Y 的逐帧驱动渲染（R1/R2/R3 均可），每帧 RGBA 像素经 IPC 送至主进程；
2. 主进程：`spawn(ffmpeg, ['-f','rawvideo','-pix_fmt','rgba','-s','WxH','-r',fps,'-i','-', …编码参数…])`，
    把帧写进 `child.stdin`（与 `video_converter.js` 既有 ffmpeg 封装同模式，含 `cancelAllFFmpeg` 取消）；
3. 编码参数：SDR WYSIWYG → `libx264/yuv420p` 或 `libvpx-vp9/libaom`；
   HDR 直出 → `libx265 -pix_fmt yuv420p10le -x265-params colorprim=bt2020:transfer=smpte2084:colormatrix=bt2020nc`
   或 HLG（`transfer=arib-std-b67`），再用现有 `mp4_hdr.js injectHdrBoxes` 写入 mdcv/clli/colr（乃至
   `st2094_50_inject.js`）——**Electron 侧全部能力现成，无需新依赖**；
4. 输出路径 `dialog.showSaveDialog`，进度事件回传（帧计数/`-progress`），完成可「在对比舞台打开」。

### 3.4 参数冻结与快照

- `let exportSnapshot = buildState()`，导出期间所有控件 `disabled`，右上角显示
  「导出中 · 参数已冻结：PQ/BT.2020 → sRGB · ACES · 100nits …」；
- 帧源、分辨率、LUT 全部取自快照，避免 `currentState` 在导出中被 `renderAll()` 改写；
- `showClamped` 按 `keepClampedMarkers` 决定是否在快照中置 false（默认保留 → 导出即所见）。

### 3.5 帧同步细节（与「同一播放控件」的关系）

- 导出期间播放控件照常可用（可先试播检查），但「导出」按钮按下后进入独占模式；
- X 路径的帧序由 RVFC 保证 = 用户看到的播放序 → 导出时间轴 == 预览时间轴；
- Y/Z 路径逐帧 seek 时暂停 UI 播放，避免双驱动抢 `currentTime`；完成后恢复当前位置。

### 3.6 两种导出语义（需要用户认知清楚，UI 上明确二选一）

| 模式 | 输出 | 语义 |
| --- | --- | --- |
| **A 所见即所得（默认）** | SDR（或画布所见）视频，像素=重渲染面板 | 把"当前参数下的观感"固化为普通视频，任何设备打开一致 |
| **B HDR 直出（P2，Electron）** | 10bit HDR10（PQ/BT.2020 + mdcv/clli）或 HLG，画面=按预览内容参数 + OOTF 后的线性场景 | "预览参数"用于内容端（TF/色域/峰值），适合把调整结果发布为真实 HDR 文件；与预览的色调映射观感需在 HDR 显示器上验证（预览即其模拟） |

> 一句话：**A = 导"所看"，B = 导"参数"**。两者共用一行参数 UI，避免用户混淆：A 用「渲染」
> 区参数（色调映射/显示端），B 用「内容」区参数（传递函数/色域/峰值）+ 峰值/参考白。

### 3.7 验收标准（表格）

| 项 | 断言 |
| --- | --- |
| 像素一致 | 导出文件解码第 K 帧 vs 录制时 `mediaRenderCv` 第 K 帧，|Δ|≤ 编码损失阈值（8bit: ≤3/255，测试用无损 VP9 CRF0） |
| 时间轴一致 | 导出时长与 [start,end] 差 ≤ 0.1s；随机抽 5 个时间点的帧序一致 |
| 参数冻结 | 导出中改控件无效；结束恢复 |
| 取消 | 取消后无半截文件；可重新导出 |
| 回验 | 导出文件自动加载为「源 B」，与源 A 用同一进度条同步播放（见 §4） |
| 自动化 | `tests/` 新增冒烟：jsdom/无头验证 X 路径主流程 + chain 数值断言（沿 RENDERING.md §6 既有方法） |

### 3.8 预览参数与输出参数一致性（WYSIWYG 可达性分析）

**结论先行**：
- ✅ **可以做到**：在「画布字节级 + 同一渲染链 + 同一页面/浏览器」范围内，导出像素与预览像素
  **100% 一致**（含无损编码时）。
- ❌ **做不到（原理性边界）**：「人眼/屏幕级」与「任意播放器回放级」永远无法与页面内预览逐像素一致
  （显示器色彩管理、播放器/操作系统二次色彩转换、观看环境均不可控）。

**一致性五层（逐层定义可达性与手段）**

| 层级 | 定义 | 可达性 | 手段 / 约束 |
| --- | --- | --- | --- |
| L1 参数级 | 导出所用参数集 == 预览参数集 | **100%** | 导出开始瞬间 `buildState()` 快照 + 冻结控件（§3.4） |
| L2 数学级 | 导出像素函数 == 所见像素函数 | **100%**（条件：统一链） | **导出帧源 = 所见面板的帧源**：GL 面板 → GL 读回（R2）；CPU 面板 → CPU 链（R3）；画布直录（R1）天然同源 |
| L3 编码级 | 文件解码后像素 == 画布像素 | 有损≈（量化）；无损=100% | 高码率/无损编码；8bit 4:2:0 时 Δ≤±3/255 为"一致"阈值；导出后自动对账（§3.8.3） |
| L4 回放级 | 任意播放器呈现 == 页面呈现 | **原理性不可能**，只能逼近 | 写入标准色彩标签（MP4 colr / WebM VUI / CICP + sRGB 或 BT.709），规范播放器尽量接近；无标签会被当作 bt.601（偏差大）→ 必须写 |
| L5 屏幕级 | 人眼所见 == 任何设备呈现 | **不可能** | 显示器/环境/观看者依赖，超出软件可控范围 |

**要达成 L1–L3，必须先修现有两处"参数未被执行/双链不一致"**（这是与当前代码的差距）：

1. **缺陷 A：GL 主链不使用色调映射参数**（`HDR_GL_FS` 只有 headroom 缩放 + clamp，`tm=none/reinhard/aces`
   在 GPU 媒体面板被忽略；CPU 回退链却执行）。后果：GL 可用时，用户调 `tm` 看不到任何变化——
   "预览参数"本身就没被完整执行。
   - 取舍 a（推荐）：给 `kColorFunctionGlsl`（或 `HDR_GL_FS`）增加 3 种 tonemap 实现（~30 行，
     与 CPU 链公式一致）→ GL 面板真正执行全部参数，L2 成立；
   - 取舍 b（妥协）：媒体面板禁用 `tm` 下拉 + UI 标注"媒体 GPU 链为直出语义"（上游同款行为）。
2. **缺陷 B：CPU 链与 GLSL 链数值体系不同**（CPU 为 D65 白点推导矩阵 + 固定 `Y^0.2` OOTF +
   soft/clip 超色域处理；GLSL 为上游 XYZ-D50 常量矩阵 + 自适应 γ OOTF + clamp）。
   - 取舍 a（推荐）：确立 **CPU 链为规范链**（它实现了全部参数），GLSL 对齐最高感知项——
     HLG OOTF 统一为固定 `Y^0.2`（或两者都标注差异）；矩阵差异（D65 vs D50，<1% 量级）留档不作强制；
   - 取舍 b：不做数学统一，靠"导出 = 所见面板帧源"（R1 直录）规避——一致性由同源性保证，
     但 CPU/GL 两面板之间的观感差异依旧存在（建议加徽标注明当前渲染路径）。

**三种"一致"语义（UI 必须三选一，不能混）**

| 语义 | 导出内容 | 一致性定义 | 代价 |
| --- | --- | --- | --- |
| **A · SDR 所见即所得（默认）** | 显示空间画面（sRGB/γ2.4 + 色调映射烘焙） | L1–L3 可 100%，L4 靠色彩标签逼近 | 导出的是"某个显示峰值下的观感"，不是 HDR 本身 |
| **B · HDR 直出** | 10bit PQ/BT.2020（或 HLG）+ mdcv/clli | 只能定义在「HDR 显示器 + 规范播放器」上；与 SDR 预览观感**必然不同** | 预览降级为「预测器/参考图」；UI 必须明示 "HDR 直出 ≠ 当前预览" |
| **C · HDR 画布所见** | HDR 画布（extended）显示空间值截取/直出 | 在 HDR 显示器上所见≈HDR 设备回放 | 依赖显示器能力；SDR 观众不可见；导出前需 HDR 校验 |

> 推荐组合：默认 **A**（严格 WYSIWYG，页内即可自证），**B** 作为 Electron 路径的可选模式并加警示，
> **C** 暂列远期。

**其余取舍清单**

| 维度 | 一致做法（默认） | 偏离做法的代价 |
| --- | --- | --- |
| 码率/保真 | VP9 CRF 0~10 或 H.264 CRF 16~18；自动对账阈值 Δ≤3/255 | 省体积 → 可见压缩；必须降低阈值或明示"近似" |
| 色度采样/位深 | 8bit 4:2:0（浏览器编码器常态） | 需要 4:4:4 / 10bit → 只能走 Electron ffmpeg 路径 |
| 帧率 | 跟随源（RVFC 实际帧，默认） | 固定 24/30/60 → 丢/补帧，时间轴近似 |
| 分辨率 | 跟随预览帧宽（`mediaFrameScale`，默认） | 自定义分辨率 → 缩放不还原，需标注"故意偏离预览" |
| EV | 媒体链不含 EV → 无歧义（导出不含 EV） | 若要让 EV 作用于媒体，需给两链新增"媒体增益"参数（+1 参数，两链同改） |
| clamp 标记 | 默认保留粉/青标记（所见即所得） | 去掉 → 不是严格所见；提供选项 |
| 色彩标签 | 导出必须写 sRGB/BT.709 colr/VUI | 不写 → 播放器按 bt.601 解码，偏色（L4 大败） |

**对账闭环（证明"一致"的手段）**：导出完成 → 自动回载为源 B → 双源同步（§4）→
页面内逐帧比较器（max Δ / 平均 Δ 直方图）+ `tests/` 里 ffmpeg ssim/psnr 脚本；
导出信息栏记录参数快照 + 导出链类型（GL 读回 / CPU 精确 / 画布直录），事后可追溯。

### 3.9 预览参数 ↔ 导出参数（自家转换器）映射与缺口清单

目标：**hdr_preview 的参数与 Electron 导出器（`views/video.html` + `video_converter.js`）的参数
形成同一套参数模型**，两边同值同步。先清点现状：

**导出端现有参数（video.html / video_converter.js / main.js）**

| 参数 | 控件/变量 | 语义 |
| --- | --- | --- |
| 峰值亮度 | `videoHdrIntensity` / `peakNits`（400–1250，默认 500） | **内容端**：高光上限 / max-cll / npl；直接转：曝光=峰值/白点（=2^EV）；增益图：maxBoost |
| 白点 | `videoWhiteNits` / `whiteNits`（80–400，默认 203） | 内容端：SDR 参考白（归一化锚点、Eclipsa refWhite、预览校准） |
| 伽马 | `videoGamma`（0.3–3.0） | 处理端：高光掩膜/曲线增益 |
| RGB 通道 | `videoRgbRed/Green/Blue`（0.3–3.0） | 处理端：逐通道增益 |
| 输出传递函数 | `videoOutputTransfer`（auto/pq/hlg） | **内容端（输出侧）** |
| 目标色域 | `videoOutputPrimaries`（auto/bt2020/p3） | **内容端（输出侧）** |
| 转换方式 | `videoModeSelect`（direct/frames） | 处理端：单层变换 / 逐帧增益图（Rust 引擎） |
| CRF / 编码器×加速 / 输出格式 / 最大宽度 | `videoCrf` 等 | 编码端 |
| Eclipsa 组 | windowScheme/uniformWindows/sceneThreshold/minWindowSec | 元数据端：ST 2094-50 |

**映射与缺口（逐项）**

| # | hdr_preview 参数 | 导出端现状 | 结论 / 需要的动作 |
| --- | --- | --- | --- |
| 1 | 内容传递函数（9 种） | 只有**输出侧** auto/pq/hlg；**输入侧固定假设 sRGB/bt709**（`zscale pin=bt709` 写死、后端按 SDR 重建） | 缺**输入传递函数**：`auto(读 CICP)/srgb/rec709/g22/g28/lin…` → 传给后端重建与 `zscale pin=`；hdr_preview 导入时已自动检测 CICP，把该值作为默认输入 TF（预览所见=导出所解） |
| 2 | 内容色域（11 种） | 只有输出侧 auto/bt2020/p3；**输入侧固定 709** | 缺**输入色域**：`auto(读 CICP)/11 种与预览同表` → 决定 重建/转换矩阵与 `zscale pin=`；预览 GAMUTS 表直接复用 |
| 3 | 曝光 EV（-4~5） | 无显式控件；**隐含**于 曝光=峰值/白点（=2^EV） | 显式化「曝光 EV」滑块（0 默认），与峰值/白点联动（峰值=白点×2^EV）；**同时让 EV 作用于媒体链**（解码后增益，CPU/GL 两链同加，默认 0）→ 预览媒体 +EV 变亮 === 导出 +EV 变亮，所见一致 |
| 4 | 显示峰值亮度 dispPeak（80–400，默认 100） | 有「峰值亮度」（400–1250）但 **是内容端 max-cll，语义错位同名** | 重命名分组：预览「显示峰值」标注"显示（SDR 模拟）"；导出「峰值亮度」标注"内容（max-cll）"。二者是不同平面，**不能互当**；参数模型里分别为 displayPeak 与 contentPeak |
| 5 | 内容峰值（预览按 TF 硬编码：PQ=10000/HLG=1000/SDR=203） | `peakNits` 用户可设 | 冲突点：预览内容峰值随 TF 法定，导出峰值自由设（默认 500）。加「峰值模式」：`跟随内容 TF`（PQ→10000…）或 `手动`；导出建议默认 1000（HDR10 常规）+ `跟随预览` 选项 |
| 6 | 白点 203（预览硬编码在两处：GLSL headroom 分母、PQ×10000/203） | `whiteNits`（默认 203） | **预览缺此参数**：把 203 提为「参考白」参数（CPU 链引用、GLSL uniform），默认 203，与导出同变量 → 预览/导出同一锚点 |
| 7 | 色调映射 tm（none/reinhard/aces） | **完全没有** | 导出新增「显示组」：tm 用于 **SDR 色调映射预览/代理输出**（转换后的 SDR tone-map MP4 与首帧预览）——这正是预览渲染面板的语义；页内用 CPU 链（R3）逐帧生成，或 ffmpeg 滤镜等效实现 |
| 8 | 显示传递函数 dispTf（sRGB/γ2.4） | 无 | 同上归入显示组（SDR 代理的 OETF 与伽马曲线） |
| 9 | 显示色域 dispGamut（11 种） | 无 | 同上归入显示组（SDR 代理的目标色域） |
| 10 | 超色域处理 gm（clip/soft） | 无 | 同上归入显示组 |
| 11 | HLG OOTF 开关 ootf | 选 HLG 输出时 OOTF 行为未暴露（后端隐式） | 暴露「HLG OOTF」开关，预览/导出同值（预览 ootf 默认开） |
| 12 | 超范围/超色域标记 showClamped | 无（诊断叠加，不是颜色） | **不需要进导出参数**：文档说明"标记是调试叠加，导出默认关闭；如需教学导出可烘焙" |
| 13 | 预设 5 组 | 无 | 同一预设系统两端共用（HDR10 参考/直出、HLG 广播、P3 影院、SDR sRGB 各设 内容+显示+处理 全组值） |
| 14 | 帧宽（240–1280，默认 720） | `maxWidth`（0–3840，默认 0=原始） | 同名不同默认/范围：参数模型统一为 `maxWidth`（0=原始），预览默认改 0 或导出默认改 720（按需）；语义一致即可 |
| 15 | — | 伽马 / RGB 通道 | **反向缺口（导出有、预览无）**：预览渲染链加入 解码后 伽马×RGB 增益（CPU 一行 + GLSL 3 uniform），否则预览无法反映导出处理 → 预览所见=导出所得 |
| 16 | — | CRF / 编码器×加速 / 输出格式 / Eclipsa 组 | 编码/元数据端，不进预览参数；导出信息/报告记录，WYSIWYG 校验时作为 L3 质量档位 |

**结论**：预览与导出做不到面板逐项同构（预览含"显示模拟"平面，导出含"编码"平面），
但可以做到**参数全集一致**：定义三组统一参数模型 ——
`内容组`（输入/输出 TF、色域、内容峰值、白点、EV、OOTF）+ `显示组`（tm、dispTf、dispGamut、
dispPeak、gm、showClamped，供 SDR 代理与 WYSIWYG 校验）+ `处理/编码组`（gamma、RGB 增益、
mode、CRF、编码器、格式、宽度、Eclipsa，导出侧；gamma/RGB 进预览链）。
**hdr_preview 嵌入 Electron 后与导出面板双向同步（同一 state 对象）**，改预览=改导出参数，反之亦然。

**落地顺序**：P0 参数模型 + 双向同步 + 预览链补（参考白、EV-on-media、gamma/RGB）+ 导出面板补
（输入 TF/色域、峰值模式、命名标注）；P1 后端/ffmpeg 接收新参数（`zscale pin=` 动态、OOTF 开关、
EV）与 SDR 代理（显示组驱动）；P2 预设同步 + WYSIWYG 校验闭环。

### 3.10 窄移植方案：只移植「内容区」参数 + 内容区控制的媒体 HDR 渲染链（修改清单）

**决策（已定）**：预览台在代码层面把参数明确分为 **内容区**（传递函数/色域/EV · 媒体导入时按
CICP 自动填入）与 **渲染区**（显示峰值/色调映射/显示色域/显示传递函数/超色域处理/标记/HLG OOTF
显示端模拟）。只移植内容区参数及其控制的媒体链「帧 → 线性 HDR 信号」段；渲染区参数留在 hdr_preview
（SDR 显示模拟专用），不进导出。

**移植的管线定义**（内容区控制段）

```
媒体帧（容器色彩标签解码后的 RGB 码值）
  → 输入 TF 解码 EOTF（9 种之一）           ← 移植 hdr_preview eotf()/GLSL ApplyOetfInv
  → 线性 ×（内容峰值/参考白）×2^EV 曝光      ← 峰值/白点/EV（后端已有 total_exposure=峰值/白点）
  → 输入色域 → 目标色域 线性矩阵             ← 移植 XYZ 数学/gamutConvertMatrix
  →（HLG 内容端 OOTF，可选）                 ← 移植 ApplyOotfAdaptiveHlg/Y^0.2
  → 线性 HDR 信号（相对峰值）→ 后端重建/编码（PQ/HLG 10bit + mdcv/clli，复用现有 zscale/x265）
```

**逐文件修改清单**

| 文件 | 改什么 |
| --- | --- |
| `hdr_preview/index.html` | ① `state` 拆分为 `state.content={tf,gamut,ev}` 与 `state.render={...}`（渲染区字段不动）；② 标记媒体链的内容段/渲染段边界（GLSL/CPU 内注释 + 函数出口：内容段输出=线性 HDR 信号，渲染段仅预览旁路）；③ 导出单一接口 `getContentParams() / applyContentParams()`（tf/gamut/ev/派生 maxNits + 媒体 CICP 检测结果）供 Electron 同步；④ 可选：内容峰值参数化（现按 TF 硬编码） |
| `views/video.html` | ① 新增「输入信号」参数组：输入传递函数 `auto(读 CICP)/srgb/rec709/γ2.2/γ2.8/rec2020 10/12/linear`、输入色域 `auto/11 种`（选项直接复刻 hdr_preview 的 TF_NAMES/GAMUTS）；② 命名标注：「峰值亮度」→「内容峰值 max-cll」，「白点」→「参考白」；③ settings 组装加入 `inputTransfer/inputPrimaries`；④ 可选 EV 显式滑块（峰值=白点×2^EV 联动，后端 `total_exposure` 已等价） |
| `video_converter.js` | ① `probeVideo` 已读 colorTransfer/colorPrimaries（auto 用）→ 透传 `inputTransfer/inputPrimaries` 给后端与 zscale；② 收尾 zscale `pin=bt709` 写死 → 按输入色域推导（bt709/bt2020/smpte432…；470M/film/XYZ/431/22 等 zscale 不支持的枚举回退默认并在 UI 灰化，或由后端矩阵承担）；③ PNG 解码阶段确认 yuv→rgb 按容器标签转换（否则 RGB 码值 ≠ 输入 TF 语义） |
| `main.js` | 基本不动（settings 已透传）；仅如需参数校验/日志记录时追加 |
| `backend/rust/src/convert.rs` | ① `srgb_to_linear`（L34，写死 sRGB）→ 按 `inputTransfer` 选 EOTF（移植 hdr_preview `eotf()` 9 种）；② 「Rec.709 → Rec.2020 常量矩阵」（L119）→ 按 `inputPrimaries→目标色域` 动态矩阵（移植 hdr_preview `gamutConvertMatrix`/XYZ 数学） |
| `backend/rust/src/models.rs` · `server.rs` | `Settings` 增 `input_transfer: String/Option`、`input_primaries: String/Option`（`primary_srgb` 标志可退役或并入）；`server.rs` JSON 字段同步（default 保持现状：srgb/709） |
| `backend/rust/src/ultra_hdr.rs`（视频逐帧重建） | 逐帧线性化（现假定 sRGB）同样参数化；`colorspace.rs` 的 ICC 主色检测可复用于「非 auto 推导」 |
| 渲染区 | **不移植**：tm/dispTf/dispGamut/dispPeak/gm/showClamped 不进导出面板与转换管线；HLG OOTF 复选框留在渲染区（显示端模拟），导出 HLG 按规范不含内容端 OOTF（若日后要"内容端 OOTF+HLG"再单独加参数） |

**决策点**：① 输入解读放后端（改 Rust，recommend，性能与现有链路一致）还是前端 ffmpeg 预处理
（不动 Rust，但 8bit PNG 中间格式限制精度/徒增链路）；② zscale 不认识的输入色域（470M/film/XYZ/
431/22）→ 后端矩阵兜底 or 灰化。核验：移植后跑 `tests/`（verify_video_* 系列回归）+ 预览台
同参数对照（同一源的预览内容段输出 vs 转换器中间线性 HDR，用 ffmpeg 提取对比）。

> ✅ **P1 实施状态（`94bf1b3`）**：决策①已定并落地——Rust 后端 `colorspace.rs` 新增
> `InputCodec`（9 种 EOTF + 11 色域→BT.709 矩阵，D65 推导，单测覆盖默认一致性/白点守恒/PQ·HLG数值），
> `convert.rs`/`ultra_hdr.rs` 图片与视频逐帧链全线参数化（默认 None 行为与旧 sRGB 逐位一致）；
> `models.rs`/`server.rs`/`cli.rs` 增 `input_transfer`/`input_primaries`；GPU 帧泵仅默认解读可用
> （GPU FFI 固定 sRGB），其余自动走 CPU；`video_converter.js` 与 `video.html` settings 透传
> （`auto` → ffprobe 检测值）。决策②（zscale 不认识的 5 色域）由后端矩阵兜底——所有 11 色域
> 都在后端先归一 BT.709 线性，`zscale pin=bt709` 保持不动，无灰化。

### 3.11 转换前预览替换：首帧图片 → 实时视频预览（P0 落地形态）

**现状（views/video.html）**：HDR 预览区挂 `<img>` 单帧——加载时 `extract-video-first-frame` →
后端图片 HDR 链路（/preview）→ dataUrl；改参数防抖 80ms 重生成同一首帧；拖进度 `extract-video-frame-at`
按需单帧；无播放中逐帧刷新；有后端冷启动/60s 超时/重试逻辑（`renderVideoHdrPreviewOnce` L748）。

**替换形态（排版不变：左参数栏 / 右预览区，右区上下两格：源视频 + HDR 预览）**：

- HDR 预览格 = hdr_preview 媒体重渲染画布（WebGL2 主链 + CPU 回退，随 `currentTime` 逐帧刷新）
  + 传输控件（播放/暂停/进度/循环/帧宽）——播放、拖动即时响应，无后端往返；
- 源视频格保持原生 `<video>`（浏览器色彩管理参考）；**同一播放控件同时驱动 HDR 画布与源视频**
  （play/pause/seek 镜像）→ 同步对比，零漂移；
- 左参数栏新增「输入信号（内容区）」组：输入传递函数（auto=ffprobe CICP + 9 种）、输入色域
  （auto + 11 种）、曝光 EV——改动即时作用于重渲染链；`auto` 在 `loadVideoFile` 时按
  `api.probeVideo` 的 color_transfer/color_primaries 自动填入（沿用现有探针，替代 hdr_preview
  的容器解析）；
- 重渲染链内显示端参数取**固定默认**（tm=ACES、dispTf=sRGB、dispGamut=BT.709、dispPeak=100、
  gm=clip、标记关、HLG OOTF 开）——「渲染区参数留在 hdr_preview 工具内」既定决策；
- 转换侧 wiring：`settings.inputTransfer/inputPrimaries/ev` 先行存字段（P1 后端/Rust 再用），
  内容峰值/参考白命名标注。

### 3.12 实测结论：Electron 33 的 HDR 画布 API（CanvasHDR feature 门控）⚠ 重要

**结论（实测于 Electron 33.4.11 / Chromium 130 / Windows，`dynamic-range: high=true`）**：

| 探测项 | 默认 | `appendSwitch('enable-features','CanvasHDR')` 后 |
| --- | --- | --- |
| `HTMLCanvasElement.configureHighDynamicRange` | **`undefined`（API 不存在）** | `function`，`{mode:'extended'}` 不抛错 |
| `navigator.hdr` | `undefined` | 仍 `undefined` |
| `gl.drawingBufferStorage(RGBA16F)` | 存在 | 存在 |
| `gl.drawingBufferColorSpace='display-p3'` | `srgb` 可改 | 可改 |

**推论**：
1. Electron 33 中 HDR 画布 API 被 Chromium 的 `CanvasHDR` feature 门控且**默认关闭**；Chrome/Edge 默认开启，
   这就是「hdr_preview 在 Chrome 里双击能看到 HDR 画布效果、在 Electron 里永远 SDR 画布（>1 高光被 8bit
   画布硬件钳制）」的根因。
2. 修复：`main.js` 在 `app ready` 之前 `app.commandLine.appendSwitch('enable-features', 'CanvasHDR')`（已落地，`1ebb6cf`）。
3. Electron 33 **未暴露 `navigator.hdr`**（WICG HDR capability）→ 原生 headroom 检测在 33 上不可行，
   除非升级 Electron（新版需重新探测）。
4. 验证方法：`node_modules/electron/dist/electron.exe --no-sandbox <probeApp>` 内 `executeJavaScript`
   探测 API 类型与调用结果（复用 `dynamic-range: high` / `color-gamut: p3` matchMedia）。

---

## 4. 同步对比整合（保留 + 增强）

### 4.1 保留现状（零改动）

- 单 `mediaVideo` → 原生 + 重渲染两画布，播放/暂停/进度/循环共用——已满足"同一播放控件控制
  两个视频进度"。

### 4.2 增强：双源 A/B 回验（导出闭环的关键一环）

```
mediaTransport（抽象：duration/currentTime/play/pause/seekTo/isEnded/onFrame）
   ├─ 源 A：原始媒体（mediaVideo 本体）
   └─ 源 B：导出文件（新 audio/video 元素，Blob URL）
```
- 单一播放控件（现有"播放/暂停/进度/循环"）驱动 **N 个源**：播放时对每个源 `play()`，
  seek 时对每个源设 `currentTime`，`timeupdate` 用**主源**（A）驱动滑块（同 Electron
  `vidCmpStage` 的做法）；
- 视图：三格并联——「源 A 原生」/「源 A 重渲染」/「源 B」（重渲染参数可为导出快照）；
  或切到「并排 / A/B 滑杆」两种对比形态（复用 Electron video.html 已有交互）；
- 本页内先实现「源 A vs 源 B」并排；Electron 整合后直接落到其 `vidCmpStage`（已有双视频叠放+分隔线+同步进度条）。

### 4.3 与 Electron video.html 的关系

- Electron 侧已有：SDR 源视频 ↔ HDR 预览（转换后 MP4）+ A/B 对比舞台（双 `video` + 单进度条）。
- 整合后职责划分：`hdr_preview` 负责**转换/导出前的参数摸索与 A（SDR-WYSIWYG）导出**；
  video.html 继续负责 **B（HDR10 直出）转换**；两者输出互为对照入口（A 的结果可拖入 B 的对比舞台）。

---

## 5. 文档勘误清单（README / RENDERING 不准确之处 → 修正）

| # | 位置 | 原文（要点） | 问题 | 修正稿 |
| --- | --- | --- | --- | --- |
| 1 | README L37 | 「本页只解析首样本并输出摘要，**渲染走 2D canvas CPU 链**，面向教学演示」 | 与 README L26、RENDERING.md §3 自相矛盾：媒体主链是 WebGL2 GPU（`pumpMediaFrameGl`），CPU 只是失败回退链 | 「本页只解析首样本并输出摘要；媒体渲染以 WebGL2 GPU 链为主、2D canvas CPU 链为回退，面向教学演示」 |
| 2 | README L42 | 「但为单文件教学工具：**单面板**、无原生 headroom 检测…」 | 媒体区实际为两格（原生参考 + 重渲染），"单面板"指单个 HDR 渲染面板，易误解 | 「媒体区为双预览格（原生参考 + 单实例 WebGL2 重渲染）、无原生 headroom 检测（navigator.hdr 未接入）…」 |
| 3 | README L38 | 「**浏览器只会把解码帧转换到显示色彩空间后才交给页面**（SDR 屏即 sRGB）…」 | 仅对 CPU 回退路径成立；GPU 主链用 `createImageBitmap({colorSpaceConversion:'none'})` 已拿到未转换帧 | 按路径拆分表述：「CPU 回退链拿到的是浏览器显示转换后的帧（SDR 屏即 sRGB），对 HDR 素材是近似重解码（SDR 素材则精确）；GPU 主链通过 'none' 转换获得未转换帧」 |
| 4 | RENDERING.md §3 / L98 | 「γ = 1.2 + 0.42·log10(L_W/1000)（BT.2408 Note 5f）」，GLSL 条件 `L_W >= 400.0 \|\| L_W <= 2000.0` | 该条件按 BT.2408 应为「400–2000 nits 区间内用线性式、区间外用扩展式」，`\|\|` 使区间外恒走线性式；**但这是上游 hdr_renderer.ts 原文（上游同款 `\|\|`），本页属逐字搬运**，且本页显示峰值滑块 80–400 恒走线性式，实际无影响 | 在 RENDERING.md 标注：「该条件与上游一致（继承其写法）；BT.2408 语义应为区间 [400,2000] 内线性式、区间外扩展式，本页峰值范围不触发差异」 |
| 5 | README L43-44 | 「kColorFunctionGlsl 逐字搬运 + BaseWebgl2Renderer 模式」 | 基本属实，但自适应 OOTF 实际搬运自 `panels/hdr_renderer.ts`（非 color_functions.ts） | 补注：「HLG 自适应 OOTF 取自 panels/hdr_renderer.ts」 |
| 6 | README L47-53「后续路线」 | 无导出项 | 方案落地后过期 | 落地后勾掉已完成项（导出、双源对比），补充「navigator.hdr、全样本动态元数据」仍为远期 |

> 另注（非勘误，但建议记录）：README L44 与 RENDERING.md §7 关于 `navigator.hdr`、逐帧动态元数据
> 未实现的表述均属实；`hdrCanvas` 徽标只证明 `configureHighDynamicRange` 未抛错，不证明显示器
> 真 HDR 能力——接入 `navigator.hdr` 后可把徽标升级为「extended + headroom」双条件显示（P3 可选）。

---

## 6. 实施阶段与验收

### P0 —— 页内 WYSIWYG 导出（hdr_preview/index.html，零依赖，本次最高优先级）
1. 导出面板 UI + 参数冻结 + 快照（§3.2/§3.4）；
2. 路径 X：RVFC 逐帧 → 导出画布 → `captureStream` + `MediaRecorder`（VP9/AV1/H.264 探测，WebM 兜底）；
3. 图片导出：单帧 PNG（顺带）；场景预览 PNG 导出（顺带）；
4. 进度 + 取消 + 下载；
5. 回验：导出完成自动加载为源 B，A/B 同步对照（§4.2）；
6. 验收：§3.7 表格 + 浏览器实测（Chrome/Edge + Electron 内嵌）。

### P1 —— 加速与确定性与测试
1. 路径 Y：WebCodecs + 移植上游 `webm.ts`（可 >1× 导出、精确帧率）；
2. 路径 R3：CPU Worker 精确链（`decodeAndRenderCode` 迁入 worker，OffscreenCanvas）；
3. `tests/`：沿 RENDERING.md §6 方法——数学段 eval 断言、GLSL glslang 编译、jsdom 冒烟覆盖
   导出主流程、ffmpeg ssim/psnr 对照脚本（参照现有 `verify_video_preview_color.js` 风格）。

### P2 —— Electron 整合（宿主应用）
1. `views/preview.html`：内嵌 hdr_preview（iframe 或直接合入 md3 布局），home.html 加入口卡片；
2. `preload.js` 暴露 IPC：`chooseSavePath / exportStart / exportFrame(RGBA) / exportDone /
   exportProgress(cb) / exportCancel`；
3. 主进程：`ffmpeg` rawvideo 编码（SDR WYSIWYG），`injectHdrBoxes`/`st2094_50_inject.js` 支持
   模式 B（HDR 直出 10bit HEVC/HLG）；
4. 与 video.html `vidCmpStage` 打通：A 导出的文件可直接拖入对比舞台做终检；
5. 打包注意：`views/preview.html`（含内嵌脚本，单文件不变）加入 `build.files`。

### P3 —— 可选增强（远期）
- `navigator.hdr` 真 headroom 探测 + 徽标升级；
- 全样本动态元数据逐帧解析并参与渲染/导出（对应上游逐帧 AGTM/HDR10+ 应用）；
- GL 读回导出（R2）以完全消除 CPU/GL 数值差。

---

## 7. 风险与已知边界（诚实清单）

| 风险 | 说明 | 对策 |
| --- | --- | --- |
| MediaRecorder 容器支持不一 | WebM(V8) 最稳；MP4/H.264 需 Chromium ≥126 | 运行时 `MediaRecorder.isTypeSupported` 探测并灰化；WebM 兜底；Electron 路径不受限（ffmpeg） |
| 录制时间戳=墙钟 | 路径 X 只能 1× 实时导出 | 需要加速时走 Y/Z；文档写明 |
| WebGL 画布直录黑帧 | 无 preserveDrawingBuffer 时 captureStream 偶发 | 统一先拷到 2D 导出画布再录（§3.3-X 第 2 步） |
| CPU/GL 数值差 | 两链 tonemap/矩阵/OOTF 不相同（§1.2） | 语义上以「所见内容」为准（R1）；确定性需求用 R3 并在导出信息中标注链类型徽标 |
| HDR 画布上录制的仍是显示空间数值 | 对 A 模式（WYSIWYG）无影响；不能冒充 HDR 直出 | B 模式只走 Electron ffmpeg 10bit 路径 |
| 帧率选择 | 源 VFR 时固定 fps 需丢/补帧 | X 默认跟 RVFC（源帧率）；Y/Z 提供 24/30/60 显式选项并在进度处显示实际帧数 |
| EV 参数 | 媒体链路不使用 EV（场景链专有） | 导出 UI 注明「EV 仅作用于场景预览」 |
| 无音轨 | 预览音频未接入（muted） | A 模式默认无音轨；Electron 路径提供「保留原音轨」选项（P2） |

---

## 8. 附录

### 8.1 关键函数/变量索引（hdr_preview/index.html）

| 符号 | 行号 | 说明 |
| --- | --- | --- |
| `state` / `buildState()` / `currentState` | 1229 / 1235 / 1761 | 参数 → 渲染状态 |
| `chainPixel` / `decodeAndRenderCode` | 1313 / 1265 | 链路 A 全链（EV→OETF→EOTF→OOTF→色域→TM→显示 OETF） |
| `tonemap` / `gamutMapSoft` | 520 / 527 | none/Reinhard/ACES；soft/clip |
| `kColorFunctionGlsl` | 1917 | GPU 共享 GLSL（上游逐字） |
| `MediaGlRenderer` / `HDR_GL_FS` | 2273 / 2213 | 链路 B（FBO/纹理/uniform/绘制） |
| `pumpMediaFrameGl` / `renderMediaFrameCpu` | 2415 / 2497 | 媒体帧泵 GL 主路 / CPU 回退 |
| `startMediaPump` / `stopMediaPump` | 2526 / 2539 | 时间驱动刷新（同步对比的机制） |
| `renderMediaFrame` | 2493 | 每帧入口（GL 优先） |
| `parseContainer` / `parseSampleMetadata` / `parseT35` | 830 / 1082 / 961 | 媒体元数据解析 |
| `loadMediaFile` | 2626 | 导入 + CICP 自动应用 + 启动泵 |

### 8.2 Electron IPC 接口草案（P2）

```js
// preload.js
api.chooseSavePath({defaultPath, filters}) → Promise<string|null>
api.exportStart(cfg /* §3.2 schema + container/codec */) → Promise<{ok}>
api.exportFrame({index, rgba: ArrayBuffer, w, h})   // 渲染器 → 主进程 → ffmpeg stdin
api.exportDone() → Promise<{path, size, frames}>
api.exportCancel()
api.onExportProgress(cb)   // {done, total, fps}
```

### 8.3 参考上游复用清单（Apache-2.0）

- `hdr-explorer/app/webm.ts` —— WebM/EBML writer（Y 路径 muxer 移植源）
- `hdr-explorer/app/download.ts` —— Blob 下载封装（可直接仿写）
- `hdr-explorer/app/app.ts` `runDynamicExport/AbortController` —— 导出 UI 状态机（进度/取消/按钮互斥）参考
- 本仓库 `video_converter.js` —— ffmpeg spawn/取消/打包路径（`resourcePath`/`RUN_CWD`）既有范式
- 本仓库 `mp4_hdr.js` / `st2094_50_inject.js` —— B 模式 HDR 元数据注入