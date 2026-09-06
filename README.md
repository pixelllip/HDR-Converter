# HDR Converter（Electron）

SDR 图片 / 视频 → HDR 的 Windows 桌面转换工具。

- **图片**：HDR PNG（Rec.2020/PQ + ICC）、HDR JPEG（ICC 增益）、**Ultra HDR JPEG**（增益图双 JPEG，Android/Chrome 可解析）
- **视频**：SDR → **HDR10 / HLG MP4**（HEVC / AV1，10-bit；输出传递函数/色域跟随「内容信号」），并可选附加 **Eclipsa（ST 2094-50 动态元数据，基带传函锁定 PQ/HLG 两选一，色域可选 P3/BT.2020）**
- 后端为 **Rust（hdrconv，唯一引擎）**；Kotlin JVM 后端已停止维护并归档（`archive/kotlin-backend/`）
- 全程可选 **CUDA GPU 加速**（像素变换 / 增益图 / NVDEC 解码 / NVENC 编码；视频逐帧重建 GPU 仅 `hdrconv video` CLI 链路接入），不可用时自动回退 CPU

Copyright © 2026 pixelllip — Apache License 2.0。部分第三方组件声明见仓库 `main` 分支的 `LICENSE` / `NOTICE` 文件。

---

## 架构总览

```
┌────────────────────────────────────────────────────────────┐
│ Electron 渲染进程（views/home·image·video.html + md3.css/js）│
│                contextBridge（preload.js）↔ IPC             │
└──────────────────────────┬─────────────────────────────────┘
                           │
┌──────────────────────────▼─────────────────────────────────┐
│ 主进程 main.js                                             │
│  · 窗口 / 对话框 / 拖拽 / 进度转发                          │
│  · 后端引擎管理：Rust hdrconv serve（唯一引擎；Kotlin 已归档）   │
│  · CUDA 检测（nvidia-smi）                                  │
│  · video_converter.js（ffmpeg 视频管道）+ mp4_hdr.js（盒注入） │
│    + st2094_50_inject.js（Eclipsa 注入）                    │
│  · Eclipsa：spawn hdrconv.exe attach-eclipsa / analyze-eclipsa │
│    / read-hdr-meta（后处理 / 动态预览预分析 / 元数据读取）     │
└───────────────┬──────────────────────────────┬─────────────┘
                │ HTTP JSON                     │ spawn
┌───────────────▼──────────────┐   ┌───────────▼─────────────┐
│ Rust  hdrconv serve（axum）   │   │ hdrconv.exe CLI         │
│  /convert /preview /estimate  │   │  · 图片批量 / 视频转换    │
│  /video-frame /batch/convert  │   │  · attach-eclipsa        │
│  /batch/progress /batch/cancel│   │  · analyze-eclipsa       │
│  /cancel /progress /status    │   │  · read-hdr-meta         │
│  /health                      │   └─────────────────────────┘
└───────────────┬──────────────┘
                │
     backend/ffmpeg（ffmpeg 9.0：libx265 / hevc_nvenc / zscale / cuvid）
     backend/cuda（CUDA 内核：hdr_gpu_ffi.dll C ABI（Rust 用）；hdr_gpu_jni.dll JNI 仅供存档 Kotlin 复现）
```

Rust 引擎通过 stdout 打印端口行 `HDR_BACKEND_PORT:<port>`，主进程轮询 `/health` 就绪后即用。Kotlin 后端已停止维护（归档于 `archive/kotlin-backend/`），不再作为回退引擎。

---

## 转换链路

### 图片

| 输出格式 | 像素处理 | 封装 |
|---|---|---|
| **HDR PNG** | sRGB→线性→RGB×曝光→伽马→Rec.709→Rec.2020→PQ 编码 | PNG + iCCP（BT.2020 ICC） |
| **HDR JPEG（jpg_icc）** | 同上 Rec.2020/PQ | JPEG + APP2 `ICC_PROFILE` |
| **Ultra HDR JPEG（jpg）** | 主图=原始输入像素（原汤化原食：按检测到的输入色彩空间标对应 ICC，sRGB/P3/2020 等；不默认转 P3），增益图=高光扩展（`gain=1+(maxBoost-1)·mask^γ`，50% 亮度以下 gain=1 保中间调） | 双 JPEG + XMP（GContainer/hdrgm）+ MPF 多图索引 + ICC |

单张 / 批量（并发 = 核心数/2+1）/ 实时预览 / 自动估算 HDR 强度（亮度直方图 99.5 分位）均可用；EXIF Orientation 自动转正。

### 视频

逐帧重建（单一模式：单层色调映射，与图片 HDR PNG/JPEG（ICC 增益）直接转链路同式——线性化 → ×RGB ×曝光（=峰值/白点） → 伽马 → Rec.2020/PQ；视频产物以 HDR10 元数据（mdcv/clli）承载，不内嵌 ICC；「转换方式」参数已移除，2026，旧逐帧增益图链路已清理）：

```
解码（NVDEC CUDA 优先 → CPU 回退）→ PNG 帧
→ 后端 /video-frame（8 并发，帧内单线程）mode=transform 单层色调映射
→ 16-bit PAM → ffmpeg 编码器 stdin（pam_pipe，延迟启动，不落盘）
→ 编码器（x265 默认 / nvenc / av1 / av1_nvenc，不可用自动降级）
→ NVENC 编码高度归一 → 合并原音频 → 注入 mdcv/clli 容器盒 → HDR10/HLG MP4
```

**Eclipsa（ST 2094-50 动态元数据，可选）**：在完成的 HDR10 MP4 上做文件级后处理——`signalstats` 逐帧 YMAX → 按基带传函换算亮度（PQ → PQ EOTF；HLG → HLG EOTF+OOTF）→ 场景切分窗（scene/uniform，镜头切阈值/最小窗时长可调）→ 每窗 MaxCLL/Hbaseline → 参考白配方载荷 → 注入 T.35 动态元数据（HEVC Annex B 按 AUD 插 Prefix_SEI；AV1 帧尾插 metadata OBU）→ remux 回 MP4。由主进程 spawn `hdrconv.exe attach-eclipsa` 执行（独立后处理，与编码引擎无关）；**前端锁定「传递函数」为 PQ / HLG 两选一（= Eclipsa 基带传函，分析端按所选基带换算亮度，其余传函回退 PQ），色域可选 P3 / BT.2020（其余回退 BT.2020）**——元数据增益应用空间随输出色域声明（P3 走通用分支 chromaticities_mode=1，BT.2020 走紧凑 C.3.8 配方），支持 HEVC/AV1，失败自动回退 HDR10。导出前可先用 `analyze-eclipsa` 对素材做逐窗预分析（为预览端动态 2094-50 渲染预生成窗口表）。

---

## GPU 加速层次

| 环节 | 实现 |
|---|---|
| 图片 Rec.2020/PQ 变换、sRGB→P3、增益图计算 | Rust FFI → CUDA 内核（`backend/cuda/`，`hdr_gpu_ffi.dll`） |
| 视频帧重建（16-bit PAM 单层色调映射） | Rust FFI 异步帧管线（`FramePump`：pinned 双缓冲 + 多槽 stream，`HDRCONV_GPU_SLOTS` 可调）；**仅 `hdrconv video` CLI 链路接入，Electron（/video-frame）仍走 CPU** |
| 视频解码 | ffmpeg NVDEC（`-hwaccel cuda`，失败回退软解） |
| 视频编码 | NVENC（`hevc_nvenc` / `av1_nvenc`） |

Rust GPU 需 `cargo build --features gpu` 且 `HDRCONV_GPU=1`；内核为 float32，与 CPU float64 逐位对齐契约略有 ±1（8-bit）/ 数十（16-bit）级差异。

---

## 目录结构

```
main.js                 主进程（窗口/IPC/引擎管理/Eclipsa 后处理）
preload.js              contextBridge API
video_converter.js      ffmpeg 视频管道（解码/逐帧重建/编码/合音频）
mp4_hdr.js              MP4 mdcv/clli 容器盒注入
st2094_50.js            ST 2094-50（Application #5）载荷编码 / SEI 组装
st2094_50_inject.js     Eclipsa 元数据附加（JS 链路：分析/分窗/注入/remux）
hevc_inject.js          HEVC Annex B 按 AUD 注入 Prefix_SEI（st2094_50_inject 依赖）
views/                  home·image·video 三页 UI + md3.css/js + hdr_preview_media.js（实时预览）
hdr_preview/            独立单文件 HDR 预览演示页（hdr-explorer 风格，预览链路原型）
assets/                 图标 + ICC（2020_profile.icc / display_p3_*.icc）
backend/
  rust/                 Rust 后端（axum HTTP + CLI，唯一引擎）
    src/server.rs       axum HTTP 端点（/convert /preview /estimate /video-frame /batch/* …）
    src/video.rs        视频 → HDR10 全流程（解码/逐帧重建/编码/合流/mdcv+clli）
    src/ultra_hdr.rs    Ultra HDR 增益图编码 / 逐帧单层色调映射 / 强度估算
    src/st2094_50.rs    ST 2094-50（Application #5）载荷编码
    src/eclipsa.rs      逐窗动态元数据注入（signalstats/AnnexB/SEI/OBU/remux）
    src/gpu.rs          CUDA C-ABI FFI + 异步帧管线（FramePump）
  cuda/                 CUDA 内核 + FFI DLL 构建脚本（jni/build_ffi.bat → hdr_gpu_ffi.dll）
  ffmpeg/               ffmpeg 9.0（libx265/hevc_nvenc/zscale/cuvid）
archive/
  kotlin-backend/       已停止维护的 Kotlin JVM 后端存档（代码/构建脚本/jar，仅供复现旧产物）
docs/                   可行性/集成决策文档（Android·Flutter、HDR 预览集成计划）
tests/                  后端/链路回归与验证脚本
MEMORY.md               项目记忆（架构决策、踩坑、待办）
```

---

## 构建与运行

### 开发运行

```bash
npm install
npm start          # electron .，热重载 views/ 与 preload.js
```

> 首次启动会拉后端：Rust `hdrconv serve`（唯一引擎；Kotlin 已归档不再回退）。`dist/win-unpacked/` 为已打包解包目录，可直接运行。

### 打包

```bash
npm run dist       # electron-builder --win portable → dist/HDR-Converter-<ver>.exe
```

打包内容（`package.json` build.files）：`main.js` / `preload.js` / `video_converter.js` / `mp4_hdr.js` / `st2094_50.js` / `st2094_50_inject.js` / `hevc_inject.js`、`views`、`assets`、`backend/ffmpeg`、`backend/cuda`、`backend/rust/target/release/hdrconv.exe`。JS 与视图留在 asar 内由 Electron 读取；`backend/ffmpeg`、`backend/cuda`（FFI DLL）与 `hdrconv.exe` 被外部进程（ffmpeg / libloading / CLI spawn）读取，必须 asarUnpack 解包。

### 后端

| 后端 | 构建 | 产物 |
|---|---|---|
| Rust（唯一引擎） | `cargo build --release`（GPU：`--features gpu`） | `backend/rust/target/release/hdrconv.exe` |
| Kotlin（已归档，仅复现旧产物） | `archive/kotlin-backend/build_backend.bat`（自动探测 JDK 17~21 + Gradle Wrapper） | `archive/kotlin-backend/build/libs/hdr-converter-backend.jar` |
| CUDA（Rust 用） | `backend/cuda/jni/build_ffi.bat`（C ABI；需 CUDA Toolkit + JDK jni.h + VS） | `backend/cuda/hdr_gpu_ffi.dll` |

> 说明：`backend/cuda/jni/build_jni.bat`（→ `hdr_gpu_jni.dll`，JNI 接口）仅供存档 Kotlin 后端复现用；现行 Rust 引擎只需 `build_ffi.bat` 产物。

### 可用环境变量

| 变量 | 作用 |
|---|---|
| `HDRCONV_GPU=1` | 启用 Rust GPU 路径（需 `--features gpu` + DLL 可加载） |
| `HDRCONV_GPU_SLOTS` | GPU 帧管线槽数（1..8，默认 2） |
| `HDR_JDK_HOME` / `HDR_GRADLE_HOME` | 仅存档 Kotlin 构建用（指定 JDK 17~21 / Gradle 缓存目录） |

---

## 测试

`tests/` 内为 Node 回归脚本（Rust 侧另有 `cargo test`，见 `backend/rust/tests/regression.rs`），覆盖：图片三格式一致性（`verify_ultrahdr.js` / `verify_image_rec2020.js` / `jpg_icc_test.js` / `roundtrip_test.js` / `check_structure.js`）、视频链路（`verify_video_convert.js` / `verify_video_direct_hdr.js` / `verify_video_rgb.js` / `verify_video_frame.js` / `verify_video_nvenc.js` / `verify_white_peak.js` / `verify_video_resolution.js` / `verify_video_preview_color.js` / `verify_video_firstframe_preview.js` / `verify_input_signal.js`）、批量与取消（`batch_test.js` / `verify_batch_cancel.js`）、GPU==CPU 对照（`gpu_cpu_consistency.js` / `verify_rec2020_gpu_cpu.js`）、EXIF 转正（`verify_exif_orientation_parser.js` / `verify_exif_orientation_preview.js`）、Rust 基线（`rust_baseline.js`）、前端内联 JS 语法（`check_inline_syntax.js` / `smoke_video_modules.js`）等。

## 相关文档

- `MEMORY.md` — 架构决策、历史踩坑、待办
- `docs/HDR_PREVIEW_INTEGRATION_PLAN_zh.md` — 导出端参数模型与 hdr_preview 媒体预览的集成计划
- `docs/ANDROID_FLUTTER_DECISION_zh.md` — Android / Flutter 移植可行性决策
- `docs/UPDATE_NOTES_vs_0.3.2.md` — 对比 0.3.2 发行版的更新说明
- `hdr_preview/` — HDR 预览台独立演示页（`README.md` 与双预览链路渲染文档 `RENDERING.md`）
- `experiments/eclipsa-st2094-50/` — ST 2094-50 / Eclipsa 可行性与 POC 研究
- `backend/cuda/BUILD.md` — CUDA 构建指南（注：含旧 Flutter 项目时期内容，以 `jni/build_*.bat` 为准）