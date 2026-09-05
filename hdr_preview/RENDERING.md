# 预览渲染链路（Rendering Pipeline）

本文档记录 `hdr_preview/index.html` 的两条预览渲染链路及其与
[hdr-explorer](https://github.com/webmproject/hdr-explorer)（上游）的对应关系，
供后续开发与排障参考。代码内注释与本文档互为索引。

---

## 0. 总览

```
                         ┌──────────── 屏幕（SDR 或 HDR 画布）────────────┐
                         │                                                │
  场景（程序化，线性 nits）  │                                                │
  SCENE Float32Array      │  链路 A（CPU）：场景预览（2D canvas）              │
  ───────────────►        │                                                │
                         │  链路 B（GPU）：媒体重渲染（WebGL2）                 │
  视频/图片帧（浏览器解码）  │  ┌ 帧源：createImageBitmap('none') → RGBA16F 纹理 │
  ───────────────►        │  └ shader: 解码→OOTF→色域→headroom→显示OETF      │
                         └────────────────────────────────────────────────┘
```

两条链路共用同一套「内容参数」（传递函数/色域/峰值/色调映射），均由
`state`（UI 控件）→ `buildState()` → `currentState`（渲染状态对象）驱动，
任意控件变化 → `renderAll()` 重算并刷新。

---

## 1. 渲染状态（buildState）

| 字段 | 来源 | 说明 |
| --- | --- | --- |
| `tf` / `gamut` | 内容下拉 | 内容传递函数 / 内容色域（9 种 TF + 11 种色域） |
| `ev` | 曝光滑块 | 场景亮度乘数 `2^EV` |
| `maxNits` | 派生 | 内容峰值：PQ=10000、HLG=1000、SDR 类=203 |
| `oeLut` / `eoLut` | 派生 | 4096 点 OETF / EOTF 查找表（按 `tf`） |
| `dispTf` / `dispLut` | 显示下拉 | 显示端传递函数（sRGB / γ2.4）及查找表 |
| `dispGamut` / `conv` | 显示下拉 | 显示色域；内容→显示色域 3×3 线性矩阵 |
| `dispPeak` | 滑块 | 显示峰值亮度（80–400 nits，默认 100） |
| `tm` / `gm` | 下拉 | 色调映射（none/reinhard/aces）与超色域处理（clip/soft） |
| `to2020` / `from2020` | 派生 | HLG OOTF 用：内容色域↔Rec.2020 转换矩阵 |
| `ootf` | 复选框 | 仅 HLG 生效；CPU/GPU 链路都支持 |
| `showClamped` | 复选框 | 粉（超内容范围）/青（超显示色域）标记 |

---

## 2. 链路 A：场景预览（CPU，2D canvas）

**入口**：`renderPreview(st)` → 逐像素 `chainPixel(r,g,b,st,flags)`。

```
场景线性 nits（内容色域 RGB）
  │ ×2^EV（曝光）
  │ ÷maxNits → clamp01
  │ OETF 编码（oeLut）            ← 内容端：模拟"用所选 TF 编码场景"
  ▼
decodeAndRenderCode(cr,cg,cb,st)
  │ (b) EOTF 解码（eoLut × maxNits）→ 线性 nits
  │ (c) HLG OOTF（可选）：内容→Rec.2020 → RGB×Y^0.2 → Rec.2020→显示色域
  │ (d) 色域转换：conv 矩阵（内容→显示色域）
  │     越界 → cyan 标记（showClamped 时）
  │ (e) 色调映射：per-channel tonemap(nits/显示峰值)
  │     none=clamp01 / reinhard=x/(1+x) / aces=ACES 拟合
  │ (f) 超色域处理：soft=1/(1+0.5·(max-1)) 压缩 / clip=clamp01
  │ (g) 显示 OETF（dispLut）
  ▼
显示码值 0..255 → putImageData
```

- 场景内容：`buildScene()` 程序化生成（太阳核心 4000 nits、天空渐变、云、
  远山、地面反光、两行满饱和测试色卡），`SCENE` 为 Float32Array（560×352×3）。
- 底部灰阶标尺：1/3/10/30/100/203/1000/10000 nits 走同一链路，
  `✂` 表示超出内容峰值（编码端裁切）。
- 悬停读数：单像素场景 nits + 内容码值（含 10bit）+ 显示码值 + ≈显示 nits。
- 曲线图（EOTF / OETF）与 CIE 1931 xy 色度图均为纯 JS 计算绘制，与链路共享
  `eotf/oetf`、`XYZ_MAT/XYZ_INV`、`GAMUTS` 等数学核心。

---

## 3. 链路 B：媒体重渲染（GPU，WebGL2）——模拟上游 HDR 面板

**对应上游**：`panels/base_renderer.ts`（BaseWebgl2Renderer）+ `panels/hdr_renderer.ts`
（HdrRenderer）+ `color_helpers/color_functions.ts`（kColorFunctionGlsl）。

**入口/帧泵**：`startMediaPump` → 每帧（时间变化时）`renderMediaFrame()` →
（GL 可用时）`pumpMediaFrameGl()`。

```
视频/图片元素（video / img）
  │ createImageBitmap(src, {colorSpaceConversion:'none'})   ← 未做显示转换的帧
  │（失败回退：drawImage 到 canvas 元素）
  │ texImage2D / texSubImage2D：RGBA16F 纹理（失败 RGBA8）
  │ UNPACK_COLORSPACE_CONVERSION_WEBGL = NONE
  ▼
fragment shader（HDR_GL_FS = #version 300 es + kColorFunctionGlsl + main）
  │ 1. ApplyOetfInv(rgb, texture_trfn)          ← 按所选内容 TF 解码
  │ 2. ApplyOotfAdaptiveHlg(HLG only)：          ← BT.2100 OOTF，γ 随显示峰值自适应
  │    γ = 1.2 + 0.42·log10(L_W/1000)（BT.2408 Note 5f，L_W=显示峰值）
  │ 3. HLG:  ×exp2(target_log2_headroom)
  │    PQ:   ×10000/203（展到"扩展 SDR"，203=参考白）
  │ 4. ToDisplayWithClamping：
  │    primariesConvert(内容→显示色域)
  │    ×linear_scale(=1)
  │    ApplyOetf(rgb, framebuffer_trfn)         ← 显示端 OETF（sRGB/γ2.4）
  │    clamp(0, exp2(target_log2_headroom))
  │ 5. show_clamped：内容超 headroom → 粉；色域转换后超 → 青
  ▼
drawing buffer：drawingBufferColorSpace='display-p3'
  HDR 画布（Chromium）：configureHighDynamicRange({mode:'extended'})
    + drawingBufferStorage(RGBA16F)
  SDR 画布：普通 8bit
```

**uniform ↔ 页面控件映射**：

| GLSL uniform | 值 |
| --- | --- |
| `texture_trfn` | `TF_TO_CICP[st.tf]`（PQ=16/HLG=18/sRGB=13/Rec.709=1/γ2.2=4/γ2.8=6/Rec.2020 10/12bit=14/15/线性=101） |
| `texture_primaries` | `GAMUT_TO_CICP[st.gamut]`（709=1/470M=4/470BG=5/BT.601=6/240M=7/film=8/2020=9/XYZ=10/RP431-2=11/P3=12/CICP22=22） |
| `framebuffer_trfn` | `DISPTF_TO_CICP[st.dispTf]`（sRGB=13/γ2.4=100） |
| `framebuffer_primaries` | `GAMUT_TO_CICP[st.dispGamut]` |
| `target_log2_headroom` | `log2(显示峰值 / 203)`（默认 100 nits → ≈-1.02） |
| `linear_scale` | 1.0（未用模拟 headroom） |
| `presentation_display_peak_luminance` | 显示峰值（滑块值） |
| `show_clamped` | `state.showClamped` |

**回退链**（`initMediaGl` 或帧更新失败时）：`mediaGlFailed=true` →
`renderMediaFrameCpu()`：帧字节当作内容码值 → `decodeAndRenderCode`（同链路 A 后半段）。
失败原因显示在面板徽标（`mediaGlBadge`）。

**原生参考面板**：直接 `drawImage(video)`（浏览器自带色彩管理/色调映射），与重渲染对比。

---

## 4. 两条链路的总映射（统一视角）

```
内容码值 ──解码(TF)──► 线性（相对内容峰值）
  │ HLG：OOTF（γ 自适应/固定 Y^0.2）
  │ PQ：×10000/203（GPU 链）
  ▼
内容色域 ──primariesConvert/conv────► 显示色域（线性）
  ▼
色调映射（GPU：headroom=log2(峰值/203) 统一缩放 + clamp；
         CPU：tonemap(nits/显示峰值) none/reinhard/aces）
  ▼
显示 OETF（sRGB / γ2.4）──► 屏幕码值
```

---

## 5. 关键函数与常量索引

| 符号 | 位置/作用 |
| --- | --- |
| `buildState()` / `state` / `currentState` | 参数 → 渲染状态 |
| `chainPixel` / `decodeAndRenderCode` | 链路 A 逐像素（编码 + 解码→显示） |
| `tonemap` / `gamutMapSoft` | CPU 色调映射与超色域处理 |
| `buildScene` / `SCENE` / `SAMPLES` | 程序化场景与色度图采样 |
| `kColorFunctionGlsl` | GPU 链共享 GLSL（11 色域矩阵 + 9 TF，逐字搬运上游） |
| `HDR_GL_VS` / `HDR_GL_FS` / `MediaGlRenderer` | 链路 B（编译/纹理/uniform/绘制） |
| `TF_TO_CICP` / `GAMUT_TO_CICP` / `DISPTF_TO_CICP` | 页面选项 → CICP 枚举 |
| `pumpMediaFrameGl` / `renderMediaFrameCpu` | 媒体帧泵 GL 主路/CPU 回退 |
| `initMediaGl` / `updateMediaGlBadge` | GL 初始化与状态徽标（含失败原因） |
| `parseContainer` / `parseSampleMetadata` / `parseT35` | 媒体元数据（容器 colr/mdcv/clli、HDR10+、2094-50） |

## 6. 验证方法

- **数学/解析**：从页面脚本提取数学核心段，在 Node 中 eval 后做数值断言
  （TF 往返、色域矩阵白点/互逆、合成与真实 MP4/WebM/SEI/OBU 解析）。
- **GLSL**：`glslang-validator-prebuilt` 的 `glslangValidator` 对
  `HDR_GL_VS`/`HDR_GL_FS`（含 `kColorFunctionGlsl`）真实编译（`-S vert/frag`）。
- **冒烟**：jsdom + Canvas/WebGL2 stub，覆盖启动、控件、GL 成功/回退路径、
  像素级画面断言（太阳核心近白）。
- **浏览器实测**（最终）：Chrome/Edge 导入 `hdr-explorer/data/*.mp4`，
  观察徽标（`WebGL2 · HDR 画布(extended)` / `SDR 画布` / `回退 + 原因`）。

## 7. 已知限制（诚实清单）

- `createImageBitmap('none')` 与 HDR 画布 API 依赖 Chromium；非 Chromium 自动走
  SDR/回退路径（与上游"主要在 Chrome 测试"一致）。
- 未接入 `navigator.hdr` 原生 headroom 检测；headroom 由「显示峰值」滑块推导。
- GPU 链的 PQ 素材假设纹理值为真 PQ 码值（未经显示转换）；若浏览器行为有差异，
  右路画面可能偏离真实 HDR 意图——与上游同构，属浏览器实现范畴。
- 动态元数据（HDR10+ / 2094-50）仅解析首视频样本并显示摘要，未逐帧关联时间轴。