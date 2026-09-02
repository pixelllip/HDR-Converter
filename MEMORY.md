# 项目记忆（MEMORY）

> 本文档记录本项目（HDR Converter Electron）的关键架构、已踩过的坑、以及历次优化的决策流程，供后续开发快速回顾，避免重复踩坑。

## 项目概览

- **用途**：图片（HDR PNG / Ultra HDR JPEG）与视频（SDR→HDR10）转换工具（Electron 桌面应用）
- **架构**：
  - 主进程 `main.js`（窗口、IPC、后端进程管理）
  - `video_converter.js`（视频转换：ffmpeg 封装 + 逐帧调用 Rust 后端）
  - 前端三页 `views/home.html`、`image.html`、`video.html` + `md3.css/js`
  - **Rust 后端 `backend/rust/`**（hdrconv：CLI + `serve` HTTP 服务，端口自动选择，`HDR_BACKEND_PORT:xxxx` 输出），**当前唯一引擎**，负责图片/视频逐帧的 HDR 重建计算
  - ~~Kotlin 后端 `backend/kotlin/`~~：**已停止维护，归档到 `archive/kotlin-backend/`**（2026，原代码/构建脚本/jar 均保留在存档，可复现旧产物；运行不再使用）
  - ffmpeg 9.0 essentials（`backend/ffmpeg/`，含 libx265 / hevc_nvenc / zscale / cuvid）
  - CUDA：`backend/cuda/`（图片链路 GPU 加速；**视频逐帧重建尚未 GPU 化，待办**）
- **开发模式热重载**：保存 `views/` 下文件或 `preload.js` 自动刷新窗口；**改 `main.js`/`video_converter.js` 需重启应用**；改 Rust 需 `cargo build --release`（backend/rust）重建 hdrconv.exe 后重启（或直接用 serve 人肉验证）

## 构建后端（重要——Rust 唯一引擎）

- **Rust 引擎构建**：`cd backend/rust && cargo build --release` → `backend/rust/target/release/hdrconv.exe`（Electron 运行时与 CLI 均用此 exe）
- ~~Kotlin 构建脚本~~（`build_backend.bat` → `backend/build_backend.ps1`）已随 Kotlin 一起归档到 `archive/kotlin-backend/`，**仅用于复现旧产物**，正常运行不再需要
- 存档内记录的 PowerShell 5.1 坑（脚本必须 UTF-8 带 BOM、`ValueFromRemainingArguments` 必须是 param 块最后）与 `.gradle_fresh` 缓存说明，仅对复现 Kotlin 旧产物有意义

## 视频转换两条链路

- **链路1 直接转（direct / transform）**：逐帧单层色调映射（图片 ICC 增益式）
- **链路2 逐帧增益图（frames / gainmap，即"UltraHDR 式"）**：逐帧用后端 `/video-frame` 重建线性 HDR → 16-bit PAM → ffmpeg 编码 HDR10
- 两条链路共用 `convertVideoFrames()`（`video_converter.js`），区别仅 `transformMode`

## 逐帧链路优化历程（本次核心工作）

按时间顺序记录，每步的原因和教训：

1. **去 base64**：`/video-frame` 原以 base64 字符串返回 PAM（+33% 体积 + 编解码 CPU）。改为后端按 `outputPath` 直写 PAM 文件，响应回 `{ok}`。
2. **逐帧并发池**：原串行 for 循环逐帧等待。改为有限并发池 `FRAME_CONCURRENCY`（默认 `min(8, 核数)`）。
3. **并发模型调整**：原"前端 4 并发 × 后端每帧内部 8 线程 = 32 线程超订 8 核"。改为**帧级并发 8 × 帧内单线程**（`UltraHdrEncoder.kt` 两个 reconstruct 函数 `threadCount = 1`），8 线程恰好吃满 8 核。图片链路（`computeGainMap`/`videoDirectPreviewRgba`）仍保留帧内多线程（单次调用场景，勿改）。
4. **二进制传输 + 主进程异步写盘**：`/video-frame` 默认 `respondBytes` 直接返回原始 PAM，主进程 `httpBinary()` 收 Buffer 后异步写盘。**但注意**：异步写盘无界堆积会内存爆满（磁盘写慢于 HTTP 接收），必须**有界**——最终方案是 worker 内 `await` 写盘完成再取下一帧（在飞 Buffer 上限 = 并发数 × 帧大小）。
5. **PAM 管道化（关键）**：PAM **不再落盘**，逐帧完成后按帧序号写入 ffmpeg 编码器的 stdin（乱序帧用 Map 缓冲 + `nextIdx` 指针保序）。SSD 写入从 ~20GB/次降到 ~1-2GB（只剩 PNG 帧 + 最终 mp4）。CPU 不再等磁盘写，负载平稳。
6. **CUDA 解码**：解码阶段 `-hwaccel cuda`（NVDEC），按输入编码（`cuvidCodecs` 集合：h264/hevc/av1/mpeg2video/mpeg1video/mpeg4/vc1/vp8/vp9/mjpeg）决定是否尝试，失败自动回退 CPU 软解并打日志（`[video] 解码使用 CUDA 硬件加速（codec）` / `[video] CUDA 解码失败...`）。
7. **编码器探测**：管道流不可重放，必须在启动前确定编码器。`encoderAvailable()` 查 `-encoders` 列表 + **实际试编码一帧**（320x240 + yuv420p10le；**不能用 2x2**，低于 NVENC 最小尺寸会误判 nvenc 不可用导致回退 CPU）。

### 踩过的坑（务必记住）

| 坑 | 现象 | 修复 |
|---|---|---|
| 管道空探测 | `Could not find codec parameters ... (Video: none, none)` | **`-f pam_pipe`**（专用 PAM 管道解复用器，不依赖探测）；`image2pipe` 需要探测具体图片格式，管道空时必失败；加 probesize/analyzeduration 无效 |
| `-start_number` 给 image2pipe | `Option start_number not found` | 仅 `image2`（文件序列）支持；管道流删除该参数 |
| 编码器启动过早 | 同上管道空探测 | **延迟启动**：`feedPam` 里第一帧数据在手（`frameBuf.has(nextIdx)`）时才 `startEncoder()`，spawn 后立即写第一帧 |
| 写已关闭管道 | `Error: write EOF`（曾崩溃主进程 / 掩盖真实错误） | stdin 挂 `error` 监听；`writeStdin` 写回调**忽略 write EOF**（编码器退出的副产物），真实错误由 `close` 事件统一报告（退出码+stderr）；`pendingWrites` 集合在 error/close 时唤醒挂起写入防挂死 |
| nvenc 探测用 2x2 | `InitializeEncoder failed: invalid param (8)`，误判回退 x265 | 探测帧用 320x240 |
| 异步写盘无界 | 200 帧内存爆满卡顿 | worker 内 await 写盘（有界） |
| detached DevTools 阻止退出 | 关窗口后后端 JVM 残留 | `win.on('close')` 里 `closeDevTools()`，让 `window-all-closed` 正常触发 |
| 主进程未捕获异常 | 偶发崩溃 | `process.on('uncaughtException'/'unhandledRejection')` 全局兜底只记录不崩溃 |

## 当前逐帧链路（最终形态）

```
ffmpeg 解码(-hwaccel cuda 尝试→软解回退) → PNG 帧(落盘 tmpDir)
→ 后端 /video-frame(8 并发, 帧内单线程) 返回原始二进制 PAM
→ 主进程按序号写入 ffmpeg 编码器 stdin(-f pam_pipe, 延迟启动, 不落盘)
→ silent_hdr.mp4 → 合并原音频 → 注入 mdcv/clli 盒子 → 清理 tmpDir
```

## 前端相关

- **伽马/RGB 初值**：伽马初值 0.9→1（image/video 两页）；图片 RGB R 通道 0.96→1（已解决偏色）
- **导入新项目重置**：`resetCompareForNewProject()`（image）/`resetVideoCompareForNewProject()`（video）在导入新素材时重置对比舞台（跳回并排）+ 重置左下进度条（单张/批量/视频）
- **icon-btn 禁用态**：`.icon-btn:disabled { opacity: 0.38 }`，hover 排除 `:disabled`
- 转换中返回按钮锁定（`btnBackHome*`.disabled）

## 图片色彩空间（2026 决策：原汤化原食）

- **Ultra HDR 主图不再默认转 Display-P3**：主图像素保持原始输入，主图 ICC 按输入检测空间选定——
  优先沿用原图嵌入 ICC（JPEG APP2 / PNG iCCP，Rust 已支持 PNG iCCP 解压），否则程序化生成（sRGB/P3/Adobe/Rec.2020/DCI-P3/ProPhoto 各自主色 + 传递函数）。
- 输入色彩空间检测扩展（Rust `colorspace.rs`）：sRGB / Display-P3 / Adobe RGB / Rec.2020 / DCI-P3 / ProPhoto + Unknown（按 sRGB 假设）。
  **Rec.709 基色与 sRGB 相同**（BT.709 primaries == sRGB primaries），ICC 主色匹配无法区分 → 归并到 Srgb。
- 旧 `primary_srgb` 开关保留兼容语义（强制 sRGB 主图 ICC）。

## Eclipsa 分段参数（2026）

- 分窗策略 UI（video.html）：`scene`（镜头切）| `uniform`（均分 N 窗），已暴露镜头切灵敏度 `sceneThreshold`（0.1~1，默认 0.4）与最小窗时长 `minWindowSec`（默认 0.5s）——main.js Rust CLI 路径与 video_converter.js JS 路径均透传。
- 官方（SMPTE PCD2）不限制窗口时间粒度；scene 默认合理（与镜头对齐）。

## 待办/未做

- **视频逐帧重建 GPU 化**（第 2 项优化）：`/video-frame` 的 `reconstructLinearHdrFrame/Transform` 仍是 JVM CPU 单线程（并行度靠帧级并发）。现有 `hdr_gpu_jni.cu` 内核均输出 8-bit 图（P3/增益图/PQ），**没有输出线性 16-bit PAM 的内核**，需要新增 CUDA 内核 + JNI 绑定 + nvcc 重编译 dll + CPU 回退，工作量大且需 CUDA 工具链环境
- 图片批量转换信号量（`ConversionSemaphore.capacity = 核数/2+1`，8 核=5）：用户提过"加点工作量"，但分析后对 GPU 场景收益有限且有反效果风险，未动
- 返回主页后显示后台转换进度（跨页共享状态）：讨论过，用户暂缓
- PNG 帧落盘（解码中间产物）：若要彻底零中间落盘需把"解码→后端"也管道化，改动大，未做

## Git 提交习惯

- 本机 git 需注入 safe.directory（仓库被 BUILTIN\Administrators 拥有）：
  ```powershell
  $env:GIT_CONFIG_COUNT="1"; $env:GIT_CONFIG_KEY_0="safe.directory"; $env:GIT_CONFIG_VALUE_0="C:/Users/Administrator/Documents/Java/hdr_electron"
  ```
- 提交说明用**中文**，格式 `类型: 摘要`（fix/perf/log/docs）
