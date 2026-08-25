# HDR Converter（Electron）

SDR 图片 / 视频 → HDR 的 Windows 桌面转换工具。

- **图片**：HDR PNG（Rec.2020/PQ + ICC）、HDR JPEG（ICC 增益）、**Ultra HDR JPEG**（增益图双 JPEG，Android/Chrome 可解析）
- **视频**：SDR → **HDR10 MP4**（HEVC / AV1，BT.2020/PQ 10-bit），并可选附加 **Eclipsa（ST 2094-50 动态元数据）**
- 后端支持 **Rust（默认）与 Kotlin JVM（回退）双引擎**，HTTP 端点契约 1:1 对齐
- 全程可选 **CUDA GPU 加速**（像素变换 / 增益图 / 视频帧重建 / NVDEC 解码 / NVENC 编码），不可用时自动回退 CPU

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
│  · 后端引擎管理：Rust hdrconv serve 优先 → Kotlin JVM 回退   │
│  · CUDA 检测（nvidia-smi）、显示器峰值亮度（DXGI）           │
│  · video_converter.js（ffmpeg 视频管道）+ mp4_hdr.js（盒注入）│
│  · Eclipsa 后处理：spawn hdrconv.exe attach-eclipsa（路径1） │
└───────────────┬──────────────────────────────┬─────────────┘
                │ HTTP JSON                     │ spawn
┌───────────────▼──────────────┐   ┌───────────▼─────────────┐
│ Rust  hdrconv serve（axum）   │   │ hdrconv.exe CLI         │
│ Kotlin 后端（Ktor，回退）      │   │  · 图片批量 / 视频转换    │
│  /convert /preview /estimate  │   │  · attach-eclipsa        │
│  /video-frame /batch/*        │   └─────────────────────────┘
│  /health /progress /status    │
└───────────────┬──────────────┘
                │
     backend/ffmpeg（ffmpeg 9.0：libx265 / hevc_nvenc / zscale / cuvid）
     backend/cuda（CUDA 内核：hdr_gpu_jni.dll JNI / hdr_gpu_ffi.dll C ABI）
```

两个后端引擎通过 stdout 打印端口行 `HDR_BACKEND_PORT:<port>`，主进程轮询 `/health` 就绪后即用；启动失败自动回退另一引擎。环境变量 `HDRCONV_BACKEND=rust|kotlin` 可强制指定。

---

## 转换链路

### 图片

| 输出格式 | 像素处理 | 封装 |
|---|---|---|
| **HDR PNG** | sRGB→线性→RGB×曝光→伽马→Rec.709→Rec.2020→PQ 编码 | PNG + iCCP（BT.2020 ICC） |
| **HDR JPEG（jpg_icc）** | 同上 Rec.2020/PQ | JPEG + APP2 `ICC_PROFILE` |
| **Ultra HDR JPEG（jpg）** | 主图=sRGB→Display-P3（保留原色），增益图=高光扩展（`gain=1+(maxBoost-1)·mask^γ`，50% 亮度以下 gain=1 保中间调） | 双 JPEG + XMP（GContainer/hdrgm）+ MPF 多图索引 + ICC |

单张 / 批量（并发 = 核心数/2+1）/ 实时预览 / 自动估算 HDR 强度（亮度直方图 99.5 分位）均可用；EXIF Orientation 自动转正。

### 视频

两种逐帧重建模式（共用一条管道）：

```
解码（NVDEC CUDA 优先 → CPU 回退）→ PNG 帧
→ 后端 /video-frame（8 并发，帧内单线程）
     mode=gainmap   逐帧增益图（保中间调，图片 Ultra HDR 式）
     mode=transform 单层色调映射（图片 ICC 增益式）
→ 16-bit PAM → ffmpeg 编码器 stdin（pam_pipe，延迟启动，不落盘）
→ 编码器（x265 默认 / nvenc / av1 / av1_nvenc，不可用自动降级）
→ NVENC 编码高度归一 → 合并原音频 → 注入 mdcv/clli 容器盒 → HDR10 MP4
```

**Eclipsa（ST 2094-50 动态元数据，可选）**：在完成的 HDR10 MP4 上做文件级后处理——`signalstats` 逐帧 YMAX → PQ EOTF → 场景切分窗（scene/uniform）→ 每窗 MaxCLL/Hbaseline → 参考白配方载荷 → HEVC Annex B 按 AUD 注入 T.35 Prefix_SEI → remux 回 MP4。由主进程 spawn `hdrconv.exe attach-eclipsa` 执行，**与 HTTP 后端引擎解耦，Kotlin/Rust 引擎均可触发**；仅 HEVC 输出支持，失败自动回退 HDR10。

---

## GPU 加速层次

| 环节 | 实现 |
|---|---|
| 图片 Rec.2020/PQ 变换、sRGB→P3、增益图计算 | Kotlin JNI / Rust FFI → CUDA 内核（`backend/cuda/`） |
| 视频帧重建（16-bit PAM gainmap/transform） | Rust FFI 异步帧管线（`FramePump`：pinned 双缓冲 + 多槽 stream，`HDRCONV_GPU_SLOTS` 可调）；Kotlin JNI 同步版 |
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
views/                  home·image·video 三页 UI + md3.css/js
assets/                 图标 + ICC（2020_profile.icc / display_p3_*.icc）
backend/
  kotlin/               Kotlin JVM 后端（Ktor HTTP，Gradle fat jar）
  rust/                 Rust 后端（axum HTTP 复刻 + CLI）
    src/st2094_50.rs    ST 2094-50（Application #5）载荷编码
    src/eclipsa.rs      逐窗动态元数据注入（signalstats/AnnexB/SEI/remux）
    src/gpu.rs          CUDA C-ABI FFI + 异步帧管线
  cuda/                 CUDA 内核 + JNI/FFI DLL 构建脚本
  ffmpeg/               ffmpeg 9.0（libx265/hevc_nvenc/zscale/cuvid）
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

> 首次启动会拉后端：Rust `hdrconv serve` 优先，失败自动回退 Kotlin JAR。`dist/win-unpacked/` 为已打包解包目录，可直接运行。

### 打包

```bash
npm run dist       # electron-builder --win portable → dist/HDR-Converter-<ver>.exe
```

打包内容（`package.json` build.files）：主进程 JS、views、assets、`backend/ffmpeg`、`backend/cuda`、`backend/kotlin/build/libs/*.jar`、`backend/rust/target/release/hdrconv.exe`，均 asarUnpack 解包（外部进程读取，必须落在 asar 之外）。

### 后端

| 后端 | 构建 | 产物 |
|---|---|---|
| Kotlin | `build_backend.bat`（→ `backend/build_backend.ps1`，自动探测 JDK 17~21，默认用项目内 `.gradle_fresh/` 缓存规避系统 Gradle 缓存损坏） | `backend/kotlin/build/libs/hdr-converter-backend.jar` |
| Rust | `cargo build --release`（GPU：`--features gpu`） | `backend/rust/target/release/hdrconv.exe` |
| CUDA | `backend/cuda/jni/build_jni.bat`（JNI，Kotlin 用）/ `build_ffi.bat`（C ABI，Rust 用；需 CUDA Toolkit + JDK jni.h + VS） | `backend/cuda/hdr_gpu_jni.dll` / `hdr_gpu_ffi.dll` |

### 可用环境变量

| 变量 | 作用 |
|---|---|
| `HDRCONV_BACKEND=rust|kotlin` | 强制后端引擎（默认 rust，失败自动回退） |
| `HDRCONV_GPU=1` | 启用 Rust GPU 路径（需 `--features gpu` + DLL 可加载） |
| `HDRCONV_GPU_SLOTS` | GPU 帧管线槽数（1..8，默认 2） |
| `HDR_JDK_HOME` / `HDR_GRADLE_HOME` | Kotlin 构建指定 JDK 17~21 / Gradle 缓存目录 |
| `JAVA_TOOL_OPTIONS` | 主进程自动注入 UTF-8 编码，避免 Windows GBK 终端中文乱码 |

---

## 测试

`tests/` 内为 Node 回归脚本，覆盖：图片三种格式一致性（`verify_ultrahdr.js` / `verify_image_rec2020.js`）、批量与取消（`batch_test.js` / `verify_batch_cancel.js`）、视频链路（`verify_video_*.js`）、GPU==CPU 对照（`gpu_cpu_consistency.js`）、Rust 基线（`rust_baseline.js`）、前端内联 JS 语法（`check_inline_syntax.js`）等。

## 相关文档

- `MEMORY.md` — 架构决策、历史踩坑、待办
- `experiments/eclipsa-st2094-50/` — ST 2094-50 / Eclipsa 可行性与 POC 研究
- `backend/cuda/BUILD.md` — CUDA 构建指南（注：含旧 Flutter 项目时期内容，以 `jni/build_*.bat` 为准）