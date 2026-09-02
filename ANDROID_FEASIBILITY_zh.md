# Android 支持可行性研究报告

> ⚠️ **现状注记（2026）**：本报告撰写时桌面后端为 Kotlin（`backend/kotlin`）。此后 **Kotlin 后端已停止维护并归档**
> （`archive/kotlin-backend/`），现行桌面引擎为 **Rust（`backend/rust`，hdrconv serve + CLI）**。报告中"复用 Kotlin
> 代码"的具体路径随之偏移：桌面侧参考实现以 Rust `backend/rust/src/{convert,ultra_hdr,icc,colorspace,video}.rs` 为准，
> 存档 Kotlin 代码仅供思想对照。
>
> 目标：评估在现有 HDR Converter Electron 框架基础上，为项目添加 **Android 端**的可实现性。
> 核心依据：**Android 平台原生（官方 API）直接支持 Ultra HDR 内容**，可大幅替换本项目手工构造 JPEG 容器的方案。
>
> 状态：**可行性评估（未编码）** · 环境：本机（Windows）已具备 Android 开发全套工具链（详见下文"环境就绪度"）。
>
> **已确认的范围（用户拍板）**：
> - 首版功能＝「图片 → Ultra HDR JPEG」+「查看现有 Ultra HDR」；
> - 最低版本＝**minSdk 34（Android 14）**，**API 35（Android 15）及以上用官方 `UltraHDRImage` 完整编码**；
> - 本轮**只产可行性报告，不动代码**；实现待后续立项。

---

## 1. 结论速览（TL;DR）

| 维度 | 结论 |
|---|---|
| **可行性** | **高** —— 尤其是「图片 → Ultra HDR JPEG」这主链路，可低成本搬到 Android |
| **最大红利** | Android 官方 API（`UltraHDRImage` + `Gainmap`）原生编码/解码 Ultra HDR，**替换本项目 `UltraHdrEncoder` 手工拼 MPF/XMP/GContainer/ICC/gain-map 的工作** |
| **核心算法可复用** | `UltraHdrEncoder` / `HdrConverter` 的纯像素数学（sRGB↔线性、增益图、HDR 变换、PQ 编码、ICC 程序化生成）是**纯 Kotlin/JVM**，可直接迁移 |
| **改造成本中等** | 主要是三处解耦：① 移除 `java.awt`/`javax.imageio`（换成 `Bitmap`+官方 API）；② 去除 CUDA JNI 依赖（走纯 CPU/Android GPU 回退）；③ 去掉 Ktor HTTP 服务模型（改成进程内库调用） |
| **视频 HDR10** | **可行但要另算一个大工程**（ffmpeg 在 Android 的移植 + Media3/MediaCodec 编码 + 逐帧模型）。建议作为二期独立子项目，与图片主链路解耦 |

**一句话**：图片方向的 Android 端是「**中高可行、收益明确、与官方生态契合**」；视频方向需要单独评估资源投入。

---

## 2. 现状盘点：现有框架到底"能搬什么"

现有桌面应用 = **Electron 壳 + 前端三页 + Kotlin HTTP 后端 + ffmpeg**。对 Android 而言：

```
桌面端                                                   Android 侧
─────────────────────────────────────────────────────────────────────────
Electron 窗口 / preload / IPC        ──不要──▶   原生 UI（Jetpack Compose）
前端 views/*.html + md3.js/css       ──换──▶    Kotlin/Compose 界面
─────────────────────────────────────────────────────────────────────────
Kotlin 后端 /backend/kotlin         ──复用─▶
  ├ UltraHdrEncoder.kt  ★纯数学+容器  ──拆──▶  数学函数直接拿去用
  ├ HdrConverter.kt     ★纯数学      ──拆──▶  直接拿去用
  ├ IccInjector.kt                   ──拆──▶  （保留 或 交给官方 API）
  ├ ColorSpaceDetector.kt            ──拆──▶  直接拿去用（纯算法）
  ├ HdrGpuJni.kt  (CUDA)             ──删─▶   Android 无 CUDA → CPU/系统GPU回退
  ├ Main.kt / Models.kt (Ktor HTTP)  ──删─▶   进程内库调用，不需要服务
  └ HdrConverter 的 java.awt/ImageIO  ──换─▶   android.graphics.Bitmap
─────────────────────────────────────────────────────────────────────────
ffmpeg + mp4_hdr.js（视频 HDR10）    ──二期─▶  MediaCodec/Media3 或 FFmpegKit
```

**能直接复用的核心资产（全部是纯 Kotlin 数学，无平台依赖）：**

- `UltraHdrEncoder.srgbToLinear / linearToSrgb`（颜色转换）
- `UltraHdrEncoder.computeGainMap`（高光扩展增益图，屏蔽掉 `HdrGpuJni` 分支后是纯 CPU）：**整段纯数字运算可用**
- `UltraHdrEncoder.estimateHdrIntensity`（自动估算 HDR 强度）
- `UltraHdrEncoder.downscaleBilinear`（增益图 1/4 下采样）
- `HdrConverter.applyHdrTransform`（图片 HDR 变换，屏蔽 CUDA 后是纯数字）
- `UltraHdrEncoder.pqEncode` / `videoDirectPreviewRgba` / `reconstructLinearHdr*`（视频相关，二期用）
- 程序化成 ICC（`buildSrgbIcc` 等）——Android 可保留用，或交给官方编码器

**不可直接复用、需要替换的三处：**

| 依赖 | 现状用途 | Android 替换方案 |
|---|---|---|
| `java.awt.image.BufferedImage` + `javax.imageio.ImageIO` | JPEG 编码主图/增益图、解码输入 | `android.graphics.Bitmap` + `Bitmap.compress(JPEG)`；解码用 `ImageDecoder` |
| `HdrGpuJni`（CUDA JNI，`.dll` + `kernel`） | Display-P3 转换 / 增益图 / HDR 变换 GPU 加速 | Android 无 CUDA。**直接走 CPU 路径**（现有代码已内置 CPU 回退）；若要 GPU 另用 Vulkan/RenderScript 方案（成本高，不建议初版） |
| Ktor + Netty HTTP 服务 | 主进程 spawn jar + HTTP 调用 | 改为**进程内直接调用**（把逻辑封装成 `HdrImageProcessor` 库）；若坚持 C/S 也可在 Android 起 Kotlin/Native 或 UnixSocket，但没必要 |

> 关键判断：**现有代码的"计算内核"与"容器/IO"高度耦合**，例如 `UltraHdrEncoder.encode()` 方法里混着 `computeGainMap`（纯数学）+ `encodeJpegRgb`（ImageIO 的 `BufferedImage`）+ 手工拼 MPF/XMP 字节。
> 要想干净复用，需要一个**解耦重构**（详见 §5 建议路线），把「增益图/像素变换」抽成与平台无关的 Kotlin 纯函数，把「JPEG 编码 / Ultra HDR 容器」做成可插拔的接口。

---

## 3. Android 官方 Ultra HDR API —— 项目最大的红利

用户提示的要点正是这里：**Android 从平台层原生支持 Ultra HDR，不用我们手拼容器**。

### 3.1 API 演进与可用版本

| API Level / 系统版本 | 官方能力 | 说明 |
|---|---|---|
| **API 34 · Android 14** | **平台原生 Ultra HDR** 支持 | `ImageDecoder` 能识别"带增益图的 JPEG"（读取 MPF/增益图）；`android.graphics.Gainmap`、`Bitmap.get/setGainmap()`；HDR Activity & 显示链路自动 tone-map 渲染 |
| **API 35 · Android 15**（本机 SDK 已含 android-35/36.1） | **完整官方编码 API**：`android.graphics.ultrahdr.UltraHDRImage`（`newInstance` / `addGainmap` / `writeToFile`） | 不用再手工写 MPF + GContainer/hdrgm XMP + ICC + 增益图布局 —— **官方 API 直接生成合规文件** |
| 本机 SDK | `android-32/34/35/36/36.1` 平台，build-tools 至 `37.0.0` | 具备编译到 target API 35/36 的完整条件 |

> 需注意：`UltraHDRImage`/`Gainmap` 更高阶能力（ISO 增益图、方向、替代基色等）带 `@FlaggedApi`（需 feature flag 或 target API ≥36 默认放开）。版本行为以 Google 官方文档为准，本文结论基于源码 `android-36.1/sources/android/graphics/Gainmap.java` 的确认（`ratioMin/ratioMax/gamma/epsilonSdr/epsilonHdr/displayRatioForFullHdr/minDisplayRatioForHdrTransition/setBitmap` 等方法均已存在）。

### 3.2 对比：桌面端 vs Android 端的 Ultra HDR 编码方式

**桌面端现状（手工组装，`UltraHdrEncoder.kt`）：**
```
主图 SOI+APP0+APP1(XMP GContainer)+APP2(ICC)+DQT/SOF+DHT + APP2(MPF)+SOS…
+ 增益图 SOI+APP1(hdrgm XMP)+APP2(ICC)+… → 拼成一个 JPEG
```
- 优点：对 ffmpeg/Chromium/desktop 生态完全自控、无需 Android 依赖
- 成本：约 300+ 行字节级容器拼接（`buildXmpPrimary/buildXmpSecondary/buildMpfPayload/reorderPrimary/buildApp2Icc/buildSrgbIcc` 等），且必须持续对齐规范版本

**Android 端（官方 API，推荐）：**
```
计算基线图 Bitmap + 计算增益图 Bitmap + 填 Gainmap 元数据
→ UltraHDRImage.addGainmap(gainmap)
→ UltraHDRImage.writeToFile(out)   // 官方产合规 Ultra HDR JPEG
```
- 直接省掉：MPF 结构、GContainer/hdrgm XMP、ICC 标签、增益图 JPEG 段拼接的全部容器工作
- 平台还提供**解码**能力 → 可新增「打开并查看/导出现有 Ultra HDR 文件」功能（桌面上靠 Chromium 渲染，Android 上是 HDR 窗口原生渲染，体验更统一）

### 3.3 据此我们对现有代码的取舍建议

| 现有代码 | Android 上是否还需要 | 理由 |
|---|---|---|
| `computeGainMap`（增益图**像素**计算） | ✅ **保留复用** | 增益图数值仍是我们要算的（高光扩展 maxBoost/gamma） |
| `srgbRgbaToDisplayP3Rgba`（P3 转换） | ✅ 保留（可选） | 决定主图色彩空间；Android `ImageDecoder`/`Bitmap` 有色彩空间机制，可灵活处理 |
| 手工 MPF/XMP/ICC/容器拼接（`buildMpfPayload`、`buildXmp*`、`buildApp2*`、`buildSrgbIcc`） | ❌ **可以不用** | 交给官方 `UltraHDRImage`/`Gainmap` |
| `encodeJpegRgb/Gray`（ImageIO） | ❌ 替换 | 换成 `Bitmap.compress(JPEG)` 产出基线图与增益图 Bitmap |

> 即：**Android 端不再需要 `UltraHdrEncoder.encode()` 的容器部分，而只需要它的"数学内核"部分 + 官方 API 做容器。**

---

## 4. 两端能力对照（初版 Android 端建议范围）

| 能力 | 桌面现状 | Android 初版建议 | 备注 |
|---|---|---|---|
| 选图 | 拖拽 + 文件对话框 | **Photo Picker（Photo 安全）+ SAF** | 不申请大范围存储权限 |
| SDR 图片 → Ultra HDR JPEG | ✅ | ✅ **主目标** | 复用 `computeGainMap` 数学 |
| SDR 图片 → HDR PNG | ✅ | 初版可先不做 / 二期 | PNG 的 PQ/P3 ICC 打包无官方封装，需自建，成本高于 Ultra HDR |
| SDR 图片 → HDR JPEG（ICC 增益） | ✅ | 低优先 | Ultra HDR 已是 Android 上更"正宗"的容器 |
| 批量 / 文件夹导入 | ✅ | 批量化可选 | 移动端单张体验优先 |
| 打开/导出已有 Ultra HDR | 预览 | ✅ **天然加分** | 官方 API 解码 + HDR 窗口渲染，比桌面更顺手 |
| 视频 → HDR10 | ✅（ffmpeg） | ❌ 二期 | ffmpeg 移植 + MediaCodec 编码，工程量大 |
| CUDA/GPU 加速 | ✅（桌面 CUDA） | CPU（Android） | Android 初版走 CPU；`computeGainMap` 已含 CPU 多线程路径 |
| 实时参数（峰值/伽马/RGB/白点） | ✅ | ✅ 参数保底（峰值/伽马） | 白点/峰值默认值对齐桌面 |

---

## 5. 推荐架构与迁移路线

### 5.1 目标结构（多模块 Gradle）

```
hdr_electron/
├── backend/kotlin/               # 现有 JVM 后端（保持不变）
├── shared-core/                  # ★新增 平台无关纯 Kotlin 库
│   ├── src/main/kotlin/
│   │   └── com/hdrconverter/core/
│   │       ├── ColorMath.kt      #   sRGB↔Linear、PQ、Rec2020、P3、增益色矩阵（从 UltraHdrEncoder/HdrConverter 抽出）
│   │       ├── GainMapMath.kt    #   computeGainMap / downscale / estimateHdrIntensity（纯函数）
│   │       ├── HdrTransform.kt   #   applyHdrTransform / reconstruct*（纯函数）
│   │       ├── ColorSpaceDetect  #   输入色彩空间检测（纯算法）
│   │       └── Model.kt          #   ConversionSettings（去掉 kotlinx.serialization 依赖也可，保留也行）
│   └── 构建：kotlin("jvm") + kotlin("android") 两平台变体（同源码，depend on 各自 io 适配）
│
├── android-app/                  # ★新增 Jetpack Compose 原生 App
│   ├── app/src/main/kotlin/...
│   │   ├── HdrProcessor.kt       #   android 适配：Bitmap ↔ PixelArray、JPEG 编解码
│   │   ├── UltraHdrWriter.kt     #   official API：UltraHDRImage + Gainmap 组装
│   │   └── ui/…                  #   Compose 界面（选图 / 参数 / 预览 / 导出）
│   └── 依赖 shared-core
```

### 5.2 落地步骤（建议顺序）

1. **解耦重构（先在桌面端做，风险低**）：把 `UltraHdrEncoder`/`HdrConverter` 中的纯数学抽到 `shared-core`，做成纯函数；JPEG IO & 容器拼装留在桌面端实现（`DesktopUltraHdrWriter`），行为不变，桌面 `npm run dist` 仍通过。这一步**不影响现网**，只是内部模块化。
2. **搭 Android 工程**：新建 `android-app/`（Compose + Material3），target SDK 35/36，引入 `shared-core`。
3. **实现处理器**：`Bitmap` 解码输入 → 调 `computeGainMap`/P3 数学 → `Bitmap.compress(JPEG)` 产基线+增益图 → 官方 `UltraHDRImage`/`Gainmap` 写文件 → `MediaStore` 保存 + 分享。
4. **参数对齐**：峰值亮度/伽马/白点默认值与桌面统一；提供简单滑块。
5. **预览**：HDR Activity / 显示链路预览；或解码回 SDR 缩略预览。
6. **（二期）** 批量、HDR PNG、视频 HDR10。

### 5.3 需要新增的适配层（工作量集中在这些）

- **像素容器 IO**：桌面 `BufferedImage ↔ ByteArray(RGBA)` + `ImageIO`；Android `Bitmap.getPixels/setPixels` + `Bitmap.compress`。**两侧接口名不同，逻辑同构，写两个薄适配即可**。
- **Gainmap 元数据映射**：桌面 `GainMapMetadata`（minContentBoost/maxContentBoost/gamma/offset…）↔ Android `Gainmap`（`setRatioMin/Max/setGamma/setEpsilonSdr/Hdr/setDisplayRatioForFullHdr/…`）。数学单位要对齐（本项目用 content boost 线性倍率，`Gainmap` 用 display ratio 域，需换算——这是**唯一较需要推敲的换算点**）。

---

## 6. 风险与需要澄清的点

### 6.1 技术风险

1. **Gainmap 元数据单位的换算**：桌面 `GainMapMetadata` 的 `min/maxContentBoost`（线性比）与官方 `Gainmap` 的 `ratioMin/ratioMax`（display HDR/SDR ratio）语义略有差别，落地时要对齐。**这是编码正确性最关键的点**，需要对照 Android 解码结果做一次端到端验证（我们已有 `tests/roundtrip_test.js` / `verify_ultrahdr.js` 思路可作为 Android 侧验收模板）。
2. **主图色彩空间**：桌面 Ultra HDR 主图现为「原汤化原食」默认——**主图像素不变，主图 ICC 按输入检测空间选定**（sRGB/P3/2020 等，优先沿用原图嵌入 ICC），不再默认转 Display-P3。Android 官方渲染管线有自己的色彩管理。初版建议保持与桌面一致：主图沿用输入色彩空间 + 对应 ICC，验证两端观感一致。
3. **目标 API 与设备普及**：要拿到完整官方编码 API，需 **API 35（Android 15）**；API 34 设备只能读/渲染，编码差分需回退（要么自建容器，要么提示升级）。需明确**最低支持版本**（建议 minSdk 34，能力等级区分）。
4. **视频 HDR10 是独立大工程**：ffmpeg 的 Android 构建（neon/x86 架构矩阵）、MediaCodec HEVC HDR 编码、逐帧 PAM 管道→MediaMuxer、音频合并，工作量接近一次新产品开发。**不应与图片主链路混为一谈**。
5. **性能**：Android 手机 CPU 多线程 `computeGainMap` 对大图（如 12MP）耗时比桌面高。可做**分辨率上限 + 进度反馈 + 协程**管理，必要时用 RenderScript（已弃）/Vulkan 二期加速。

### 6.2 产品范围（**已确认**）

| # | 决策 | 结论 | 状态 |
|---|---|---|---|
| 1 | 首版功能 | **「图片 → Ultra HDR JPEG」+「查看现有 Ultra HDR」** | ✅ 已确认 |
| 2 | 视频 HDR10 | 单独二期（不在首版） | ✅ 已确认 |
| 3 | 最低 Android 版本 | **minSdk 34**（Android 14）；API 35+ 用官方 `UltraHDRImage` 完整编码 | ✅ 已确认 |
| 4 | 与桌面共用参数/预设规格 | 是，峰值/伽马/白点默认一致 | ✅ 约定 |
| 5 | 反向（Ultra HDR → SDR / 调整导出） | 可选加分项（未纳入首版） | ⏸ 待定 |
| 6 | 本轮是否动代码 | **只出报告，不动代码** | ✅ 已确认 |

---

## 7. 工作量粗估（图片主链路）

| 阶段 | 规模 | 说明 |
|---|---|---|
| ① `shared-core` 解耦重构（桌面端先行） | 中（2~4 天） | 抽纯数学，影响面小，可先验证桌面仍通过 |
| ② Android 工程搭建（Compose + shared-core + 官方 Ultra HDR） | 中（3~5 天） | 含 Gainmap 元数据换算与端到端验证 |
| ③ UI + 参数 + Photo Picker + MediaStore 导出 | 中（2~4 天） | 初版单张体验 |
| ④ 批量 / 自建容器回退 / 预览打磨 | 中（2~4 天） | 可选 |
| 视频 HDR10 | 大（另计 >2 周） | 二期 |

**初版「图片 → Ultra HDR」(含 UI 与验收) 乐观合计约 1~2 周**（一名熟悉 Kotlin/Android 的开发者，复用现有数学）。前提是先在桌面端做 `shared-core` 解耦。

---

## 8. 环境就绪度（本机实测）

本机已具备完整 Android 开发条件，无需额外装 SDK：

| 项 | 路径 / 版本 |
|---|---|
| Android SDK | `C:\Users\Administrator\AppData\Local\Android\Sdk` |
| 平台 | `android-32 / 34 / 35 / 36 / 36.1` |
| Build-tools | `34.0.0 / 35.0.0 / 36.0.0 / 36.1.0 / 37.0.0` |
| NDK | `28.2.13676358`（如需 Native） |
| Cmdline-tools | `latest` |
| Android Studio | `D:\Program Files\Android Studio`（jbr = JDK 21） |
| 现有 JDK | `~/.gradle/jdks/jetbrains_s_r_o_-21-amd64-windows.2`、Corretto 21 |
| 现有 Gradle 经验 | 项目已用 Gradle Wrapper 8.14 构建 Kotlin 后端，可复用缓存/经验 |

> 结论：从「能否构建 Android App」角度，**本机开箱即可**。

---

## 8.5 确证：Ultra HDR → HDR 视频 的官方实现（已读源码）

> 用户提供线索：[android/platform-samples PR #83](https://github.com/android/platform-samples/pull/83)，并指向 `samples/media/...`。
> 经放行网络后 clone 官方仓库核验，**真正的「Ultra HDR → HDR 视频」代码在 `samples/media/ultrahdr/src/main/java/com/example/platform/media/ultrahdr/video/UltraHDRToHDRVideo.kt`**（Apache-2.0）。
>
> ⚠️ **修正**：你给的 `samples/media/video` 实际是 **Media3 Transformer 神经风格迁移/合成**示例（`androidx.media3.transformer/effect` + TFLite，灰度/缩放滤镜）——**与 Ultra HDR 无关**。Ultra HDR→视频在 `samples/media/ultrahdr/video/` 下。

### 8.5.1 确认的官方技术栈（不依赖 ffmpeg，纯平台 API）

| 环节 | Android 官方 API（出自 UltrahdrToHDRVideo.kt） | 说明 |
|---|---|---|
| 输入 | Asset 里的 Ultra HDR JPEG（`BitmapFactory.decodeStream` 解码） | 样例从 assets 读，生产可用 Photo Picker/SAF |
| 帧承载 | `ImageWriter` + `HardwareBuffer`（`RGBA_1010102`，10bit）+ `DataSpace.DATASPACE_BT2020_HLG` + `MaxImages=32` | 10bit RGB 硬件缓冲 |
| 渲染入缓冲 | **`HardwareBufferRenderer` + `RenderNode`** 绘制 Bitmap，`obtainRenderRequest().setColorSpace(BT2020_HLG)`，同步 `image.fence` | 增益图渲染/HDR 色彩空间由硬件管线处理 |
| HDR 编码 | **`MediaCodec`** HEVC、`HEVCProfileMain10`（HLG）；换 `HEVCProfileMain10HDR10` + `COLOR_TRANSFER_ST2084` = PQ/HDR10 | Surface 输入 `COLOR_FormatSurface`，BT.2020、Full range |
| 打包 | **`MediaMuxer`** MP4，`CODEC_CONFIG` 帧时 `addTrack` + `start` | HDR 元数据由 MediaCodec/平台写入 |
| 播放/验证 | **`ExoPlayer`** 播产出 mp4，自动识别 HDR 并激活高亮模式 | 无需额外 HDR 渲染代码 |
| 门槛 | `@RequiresApi(UPSIDE_DOWN_CAKE)` = **API 34（Android 14）+** | 与 minSdk 34 计划一致 |

> 这与本报告 §8.5 早前"推断"完全一致：**确实走 `ImageWriter/HardwareBuffer → MediaCodec HEVC/HDR → MediaMuxer`，不依赖 ffmpeg**。HDR 元数据与色彩空间由平台保证，桌面端手写的 `mp4_hdr.js`（注 `mdcv`/`clli` 盒）在 Android 上可省。
>
> 关键映射（样例里直接改 2 行即可 PQ/HDR10）：
> - HLG：`ColorSpace.Named.BT2020_HLG` + `HEVCProfileMain10` + `COLOR_TRANSFER_HLG`
> - PQ/HDR10：`ColorSpace.Named.BT2020` + `HEVCProfileMain10HDR10` + `COLOR_TRANSFER_ST2084`

### 8.5.2 对两端目标的价值（确证版）

- **二期视频（Ultra HDR → HDR 视频）**：有现成官方参考实现，纯平台 API、无 ffmpeg，工程可行性高；把 `computeGainMap` 产出的 Ultra HDR 图片（或图像序列）喂给这条链路即可得到 HDR10/HLG 视频。
- **注意**：官方样例是一次图片静态转视频（单帧 60fps 循环）。要变成「图片序列→视频」或「SDR→Ultra HDR→视频」，需在其框架上按帧 `dequeueInputImage`→渲染→`queueInputImage`，原理一致。
- **与本项目主干的关系**：本项目（Electron）已能产出合规 Ultra HDR JPEG（含增益图）；Android 侧用官方栈把这些 Ultra HDR 图转成 HDR 视频，即可闭环「SDR→HDR 视频」。增益图数学仍是共享点（见 §8.6）。

### 8.5.3 官方源码要点（UltraHDRToHDRVideo.kt 关键 API 摘录）

- `ImageWriter.Builder(encoderSurface).setHardwareBufferFormat(HardwareBuffer.RGBA_1010102).setDataSpace(DataSpace.DATASPACE_BT2020_HLG).setMaxImages(32).setUsage(USAGE_GPU_COLOR_OUTPUT or USAGE_GPU_SAMPLED_IMAGE)`
- `HardwareBufferRenderer(buffer)` + `RenderNode` 画 Bitmap → `renderer.obtainRenderRequest().setColorSpace(BT2020_HLG).draw(...)`，提交后 `image.fence.awaitForever()` 再 `imageWriter.queueInputImage(image)`
- `MediaCodec` surface 输入：`FORMAT_PROFILE=HEVCProfileMain10`、`KEY_COLOR_STANDARD=BT2020`、`KEY_COLOR_RANGE=full`、`KEY_COLOR_TRANSFER=HLG`（改 PQ 见上）
- `MediaMuxer(filePath, MUXER_OUTPUT_MPEG_4)`，`CODEC_CONFIG` 帧 `addTrack`/`start`，每帧 `writeSampleData`，`signalEndOfInputStream` 结尾
- 前置检查：`HardwareBuffer.isSupported(...)` 校验 HW 加速；`MediaCodecList.findEncoderForFormat(format)` 校验设备支持 HEVC Main10

---

## 8.6 重大补充：本机已存在 Flutter 多平台工程 `hdr_convert`（Android 的实际载体）

> ⚠️ **本报告前几节是按 Electron 项目（Kotlin JVM 后端）为蓝本写的「独立 Android 移植」。**
> 现场盘点发现，**本机还有一个 Flutter 多平台工程 `C:\Users\Administrator\Documents\Java\hdr_convert`**，自带完整 Android 工程（`android/app` + Kotlin `MainActivity`）。这很可能是"Android 端"更现实的落点。下面修正结论。

### 8.6.1 Flutter 工程现状盘点

| 项 | 现状 | 影响 |
|---|---|---|
| 平台覆盖 | Windows / Web / **Android / iOS / macOS**（Flutter 多平台） | Android 是天然目标之一 |
| HDR 处理 | **纯 Dart** `image` 包（`lib/services/hdr_converter_io.dart`），条件导出 `io/js/stub` | 移动端也走同一套 Dart 路径 |
| Android 原生代码 | 仅有默认 `MainActivity`（stock Flutter 嵌入），**无任何 MethodChannel/插件桥** | 目前**接不到** Android 官方 API |
| 「Ultra HDR JPEG」输出 | **其实不是真 Ultra HDR** —— 只是把 BT.2020 ICC 注入 JPEG（`_injectIccIntoJpeg`），**没有增益图 / MPF / GContainer / hdrgm XMP** | 与 Electron 项目 `UltraHdrEncoder`（真 Ultra HDR）**能力不对等** |
| 增益图数学 | `lib/` 内**完全没有** gain-map / MPF / hdrgm 实现（grep 只有枚举标签 `ultraHdrJpeg`） | Android 端若要做真 Ultra HDR，目前是**空白** |

> **一句话**：Electron 项目有"真 Ultra HDR 数学"（Kotlin），Flutter 项目有"多平台壳 + Android 工程"但缺"真 Ultra HDR 能力"。两者正好互补。

### 8.6.2 关键难点：纯 Dart 够不到 Android 官方 Ultra HDR API

Android 的 `android.graphics.ultrahdr` / `android.graphics.Gainmap` 是**平台原生 API**，Dart 无法直接调用。要在 Flutter 的 Android 端使用官方能力，必须——

```
Dart (lib/services/hdr_converter_*.dart)                                    Android 原生
──────────────────────────────────────────────────────────────────────────────
HdrConverterPlatform.convertSdrToHdr(...)
   │
   ▼
MethodChannel(".../hdr_ultrahdr") ───────► MainActivity / Kotlin：
       │  传位图字节 + 参数                android.graphics.Bitmap
       │                                  → computeGainMap（复用数学）
       │                                  → UltraHDRImage.addGainmap(Gainmap)
       │   ←  回 Ultra HDR JPEG 字节      → UltraHDRImage.writeToFile / MediaStore
```

选择：① 用 `MethodChannel`（简单，本项目规模够）；② 或做成独立 **Flutter plugin**（可复用、多端共享）。

### 8.6.3 三重资产如何拼出一个干净的"真 Android Ultra HDR"方案

| 来源 | 提供什么 |
|---|---|
| **Flutter `hdr_convert`** | 多平台 UI + 文件选择/保存 + 参数面板 + **Android 工程壳** |
| **Electron 后端 Kotlin 数学**（`computeGainMap` / `estimateHdrIntensity` / P3 转换） | 增益图**数值**与参数语义（峰值/伽马/白点），保证与桌面观感一致 |
| **Android 官方 API**（`UltraHDRImage` + `Gainmap`） | 增益图 JPEG/容器/MPF/XMP/ICC 的**合规编码**（替换桌面手工字节拼接） |
| **官方 video 示例（PR #83 方向）** | (二期) Ultra HDR → HEVC/HDR 视频，走 MediaCodec/MediaMuxer，**不依赖 ffmpeg** |

> 即在 Android 端：**Dart 提供 UI + 编排，Kotlin(Gradle)/官方 API 提供「增益图数学 + 合规容器 + HDR 视频编码」**，通过 MethodChannel 打通。这与纯 Dart 现状不同，但**增量不大**（只需一个原生桥 + 把 Kotlin 数学里不依赖 AWT 的部分搬成 Android 可用的 Kotlin）。

### 8.6.4 需要向用户澄清/确认的（因为涉及工程选型）

| # | 问题 | 建议 |
|---|---|---|
| A | Android 端做在 **Flutter `hdr_convert`** 里（推荐），还是另起 Compose 工程（Electron 直移）？ | **Flutter**（已有壳，多平台复用） |
| B | 增益图数学放 **Dart** 重写 还是 **Kotlin/Gradle 原生**（接 MethodChannel）？ | **Kotlin 原生**（能直接喂给官方 `Gainmap`，且与 Electron 后端数学同源，最省事） |
| C | 「查看/解码现有 Ultra HDR」是否要（官方 API 有原生解码，Flutter 需同样走桥）？ | 建议要，成本低、体验好 |
| D | 视频（Ultra HDR → HDR 视频）是否纳入 | 官方实现已确证（§8.5），无 ffmpeg；建议先图片，视频二期 |

> 说明：本报告 §2~§7 面向「独立 Android/Compose 工程」仍成立（架构与难点一致）；但最省力的现实路径是 **在 Flutter 工程上加原生桥**。两者共用同一结论：**官方 API 替换手工容器、共享 gain-map 数学。**

---

## 9. 下一步建议

本轮（用户已确认）：**只出可行性报告，不改动代码**。范围＝图片→Ultra HDR + 查看现有 Ultra HDR，minSdk 34 / API 35+ 官方编码。

**选型更新（基于 §8.6 的 Flutter 工程发现）**：Android 端最省力的落点 = **Flutter `hdr_convert`（已有多平台壳 + Android 工程）上加原生 Ultra HDR 桥**，而不是另起 Compose 工程。两种思路可行性都成立，但 Flutter 复用现有 UI/壳，增量最小。

后续立项时，推荐顺序：
1. **（先决）确认 Flutter 优先选型 + 确认增益图数学放 Kotlin 原生（§8.6.4）**；
2. 在 Flutter `android/` 侧实现 **MethodChannel 桥**：Dart 传位图+参数 → Kotlin 算增益图（迁移/对齐 Electron 后端 `computeGainMap` 数学，去掉 AWT/CUDA/Ktor）→ 官方 `UltraHDRImage`+`Gainmap` 编码 → 返回合规 Ultra HDR JPEG；
3. 用「图片 → Ultra HDR」端到端验收（对齐 `verify_ultrahdr.js` 思路），重点核对 Gainmap 元数据换算；
4. **（你提供的官方线索，已确证）** 视频二期走官方 `UltraHDRToHDRVideo.kt`（§8.5）的 **`ImageWriter/HardwareBuffer → HardwareBufferRenderer → MediaCodec HEVC/HDR → MediaMuxer`**，**不依赖 ffmpeg**，用「本工程增益图产出 Ultra HDR + 官方 Ultra HDR→视频」闭环；
5. （可选）桌面 Electron 端是否同步做 `shared-core` 与 Android 原生共用同一套 Kotlin 数学，保持一致。

---

## 附：Android 官方 Ultra HDR API 关键类速查

| 类 | 位置 | 作用 |
|---|---|---|
| `android.graphics.Gainmap` | 平台（API 34+，源码已在 android-36.1 确认） | 增益图像素 + 应用元数据 |
| `android.graphics.ultrahdr.UltraHDRImage` | 平台（API 35+） | Ultra HDR 文件读/写的官方容器 |
| `android.hardware.HardwareBuffer` / `RGBA_FP16` | 平台 | HDR 高精度像素承载 |
| `android.graphics.ImageDecoder` | 平台（API 34+） | 解码时自动识别增益图并关联 `Bitmap` 增益图 |
| `android.view.Window` / Activity `COLOR_MODE_HDR` | 平台 | HDR 显示窗口声明与自动渲染 |
