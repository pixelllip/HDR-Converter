# hdrconv —— Rust 版 HDR 转换 CLI

Electron 版 HDR 转换器的 **Rust 后端（唯一引擎）**（由 Kotlin 后端 `archive/kotlin-backend` 1:1 移植而来；
Kotlin 已停止维护并归档，本模块为现行实现）。CLI 形态 + `serve` 常驻 HTTP 服务（axum 替代原 Ktor）供 Electron 调用。

## 构建与运行

```bash
cd backend/rust
cargo run -- --help              # 查看参数
cargo run -- photo.jpg -f png    # HDR PNG（Rec.2020/PQ 变换 + 2020 ICC iCCP 注入）
cargo run -- photo.jpg -f jpg-icc    # HDR JPEG（同变换 + APP2 ICC 注入）
cargo run -- photo.jpg -f jpg    # Ultra HDR JPEG（增益图 + MPF + XMP，Kotlin "jpg" 语义）
cargo run -- photo.jpg -f png --icc /path/to/profile.icc  # 自定义 ICC（默认自动探测 assets/2020_profile.icc）
cargo run -- a.jpg b.jpg -f png -j 4   # 批量（并发 = 核心数/2+1，可 -j 指定）
cargo run -- photo.jpg --check    # 只探测输入色彩空间
# 视频转换用子命令（单一模式：逐帧单层色调映射）：
cargo run -- video input.mp4 -o out.mp4 --peak 1000
cargo run -- video input.mp4 --encoder nvenc   # 单层色调映射 / 硬编
cargo test                        # 常规回归测试
cargo test -- --ignored           # Kotlin 逐像素对照（需先生成基准，见下）
```

## 视频链路（`hdrconv video`）

```
ffprobe 探测 → ffmpeg 拆 PNG 帧 → 逐帧 Rust 重建 16-bit PAM（帧级并发 ≤8）
→ 管道喂给 ffmpeg 编码器（pam_pipe + zscale 线性→2020/PQ + yuv420p10le）
→ 无声 HDR MP4 →（nvenc 时 libx265 归一 coded 补边）→ 合音频 → 注入 mdcv/clli 盒
```

- 逐帧重建固定单层色调映射（transform，与图片 HDR PNG/JPEG 直接转链路同式；视频产物为 HDR10 元数据，
  不内嵌 ICC；旧逐帧增益图链路已清理）
- 参数对齐 JS `convertVideoFrames`：`peak`=PAM 归一峰值（=峰值/白点）、`npl`/`max-cll`=峰值、
  MASTER_DISPLAY=P3(G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)L(10000000,1))、
  默认 x265 CRF20（nvenc/av1/av1-nvenc 可选，不可用自动降级回退）
- 与 JS 端差异：帧重建直接调 Rust 库（不再走 Kotlin HTTP）；解码仅 CPU 软解（JS 尝试
  CUDA NVDEC）；Eclipsa（ST 2094-50 动态元数据）未移植
- 产物验证：`ffprobe -select_streams v:0 -show_entries stream=pix_fmt,color_primaries,color_transfer`
  应为 `yuv420p10le,smpte2084,bt2020`，且 MP4 内含 `mdcv`/`clli` 盒（Chromium 依赖）

## Kotlin 逐像素回归基准

```bash
node tests/rust_baseline.js       # 生成 tests/rust_ref_input.png + tests/rust_ref_kotlin.png
                                  # （自动启动 java -jar 后端，HDR_GPU_DISABLE=1 强制 CPU）
cd backend/rust && cargo test -- --ignored   # 断言 Rust 输出与 Kotlin 零像素差异
```

## 模块 ↔ Kotlin 对照

| Rust 模块 | Kotlin 源文件 | 状态 |
|---|---|---|
| `src/cli.rs` | Electron 前端 settings（无 Kotlin 对应） | 已接线（clap） |
| `src/models.rs` | `Models.kt`（ConversionSettings / RgbAdjustment） | 已接线 |
| `src/convert.rs` | `HdrConverter.kt` | **B 曲线版**（png/jpg_icc 管线：明度增益 + 白点滚降 + 可选输出色域；已不再与 Kotlin 旧 RGB×曝光/伽马实现逐位对齐） |
| `src/ultra_hdr.rs` | `UltraHdrEncoder.kt` | **已移植**（增益图/双 JPEG/XMP/MPF/ICC 组装 + B 曲线/下采样） |
| `src/icc.rs` | `IccInjector.kt` | **已移植**（PNG iCCP / JPEG APP2；iCCP 名改为通用 `ICC`，P3/sRGB 使用程序化 PQ profile） |
| `src/colorspace.rs` | `ColorSpaceDetector.kt` | **已移植 + 扩展**（ICC 主色匹配 / EXIF / JFIF / PNG 标记；新增 Rec.2020/DCI-P3/ProPhoto、PNG iCCP 解压、返回嵌入 ICC 字节） |
| `src/gpu.rs` | `HdrGpuJni.kt` + `backend/cuda/include/hdr_gpu.h` | FFI 就绪（feature `gpu`）；**DLL 实证仅导出 JNI**（`examples/dump_exports.rs` 枚举），启用需 CUDA 侧补 C-ABI 导出 |
| `src/video.rs` | `video_converter.js`（convertVideoFrames）+ `mp4_hdr.js` | **已移植**（探测/NVDEC 硬解/拆帧/逐帧重建/编码器降级/mdcv+clli 注入/Eclipsa） |
| `src/server.rs` | `Main.kt`（Ktor 路由） | **已移植**（axum 1:1 端点契约；`hdrconv serve`） |
| `src/st2094_50.rs` + `src/eclipsa.rs` | `st2094_50.js` + `st2094_50_inject.js` + `hevc_inject.js` | **已移植**（ST 2094-50 载荷/SEI 注入） |
| `tests/regression.rs` | — | 常规测试 + Kotlin 逐像素对照（`--ignored`） |

## 移植顺序建议

1. ✅ `convert.rs::apply_hdr_rec2020_pq`（/convert 的 png / jpg_icc 管线；现行 B 曲线 + 输出色域）
2. ✅ `icc.rs`（PNG iCCP / JPEG APP2 注入；PNG iCCP 名改为通用 `ICC`，png / jpg-icc 已端到端可用）
3. ✅ `colorspace.rs::detect`（ICC 主色匹配 / EXIF ColorSpace / JFIF / PNG 标记；Rec.2020/DCI-P3/
   ProPhoto 扩展 + PNG iCCP 解压；`DetectedColorSpace` 返回嵌入 ICC 供"原汤化原食"沿用）
4. ✅ `ultra_hdr.rs`（compute_gain_map → encode_ultra_hdr；主图默认**原汤化原食**：像素不变、
   主图 ICC 按检测空间选/沿用嵌入 ICC，不再默认转 Display-P3；XMP 数值与 Kotlin 完全一致，
   JPEG 编码器不同 → 字节流不一致属预期；jpg = Ultra HDR 语义已对齐）
5. ✅ `video.rs`（视频 → HDR10：ffprobe/NVDEC/拆帧/逐帧重建/编码/合流/mdcv+clli；验证
   `yuv420p10le,smpte2084,bt2020` + 首帧线性峰值 ≈ 峰值亮度）
6. ✅ `server.rs`（axum HTTP 服务，1:1 复刻 Kotlin 端点契约；`hdrconv serve` → `HDR_BACKEND_PORT:<port>`）
7. ✅ 视频 NVDEC 硬解（cuvid 支持集 + 失败回退 CPU）
8. ✅ Eclipsa（`--eclipsa`：signalstats 逐窗 MaxCLL → 参考白配方载荷 → 按 AUD 注入 Prefix_SEI）

## 待办（后续阶段）

- ~~Electron 主进程切换~~：`main.js` 换 spawn 目标（见 rust-backend 分支的引擎切换提交）
- ~~CUDA C-ABI~~：**已接通**（`backend/cuda/jni/hdr_gpu_ffi.cu` + `build_ffi.bat` →
  `hdr_gpu_ffi.dll`；`cargo build --features gpu` + `HDRCONV_GPU=1` 启用）。
  **2026-10 图片链路已与 CPU 同式**（B 曲线 + 输出色域 + Oklab 色域拓展；增益图含局部镜面高光），
  4K 实测 CPU 471ms → **GPU 33ms（14.4×）**，增益图 1099ms → 202ms（5.4×）；
  float32 内核与 CPU float64 的 8-bit 差异 ≤1（约 0.02% 字节）
- **4K 视频 GPU（2026-08 实测，60 帧 3840×2160）**：
  - 逐帧重建：transform16（单层色调映射）GPU 快 **13.7~14.4×**（1001→69ms）
  - 全链路（拆帧+重建+x265）：CPU 10.0s vs **GPU 7.2s（快 28%）**，输出 **PSNR=∞（逐位一致）**
  - **内存/硬盘观测**：GPU 峰值内存 +723MB（4K：4 槽 pinned 双缓冲 ≈330MB + 有界通道 PAM 积压 ≤300MB
    + 帧缓冲），**磁盘零增量**（tmp 帧目录与输出同 CPU 路径；无额外落盘）
  - **槽数缩放曲线**（`HDRCONV_GPU_SLOTS=1/2/4/8`，60 帧 4K，输出均 PSNR=∞）：
    | 槽数 | 1 | 2 | 4 | 8 | CPU |
    |---|---|---|---|---|---|
    | 耗时 | 7.6s | 7.5s | 7.2s | 7.3s | 10.4s |
    | 峰值内存 | 726MB | 1225MB | 1394MB | 1700MB | 663MB |
    → 1 槽即达 ~73% 收益，2~4 槽封顶（SM/PCIe 饱和），内存随槽数近线性涨；**默认 2**（双缓冲），
    GPU 内部并行已打满，host 线程数无关紧要（旧 8 线程同步路径因 legacy stream 隐式同步实际串行）
  - JIT 已消除：build_ffi.bat 双 gencode（compute_75 PTX 兜底 + **sm_89 cubin**），RTX 40 系零运行时 JIT
  - 泵参数（槽数/通道容量）为常量可调：`FRAME_SLOTS=4`、`sync_channel(6)`
- 性能基准：`tests/tmp_bench*.{rs,js}`（gitignored）

## 注意事项

- 参数语义对齐：总曝光 = 峰值/白点（2^EV），默认峰值 500、白点 203（Electron UI / CLI 统一；
  Kotlin 后端回退默认是 1000/203，调用时务必显式传参）；**白点滚降系数 `--rolloff`（0~1，默认 0.45）**
  与 **ICC 输出色域 `--output-gamut 2020|p3|srgb`** 为 ICC / Ultra HDR 共用/新增参数
- 格式语义：`jpg` = Ultra HDR（增益图链路），`jpg-icc` = ICC 增益（明度 B 曲线 + 输出色域），
  `png` = HDR PNG（同上 + 对应输出色域 ICC）
- **对齐粒度说明**：png / jpg-icc 已改为明度 B 曲线 + 输出色域，**不再与 Kotlin 旧实现逐像素一致**；
  ultra-hdr 的主图/容器/XMP 结构对齐，但增益图曲线同为 B 曲线（中调抬升 + 白点滚降 + 局部镜面高光，
  Rust 为准；Kotlin 归档仍是旧的高光阈值实现）。JPEG 编码器不同，字节流不可能逐位一致；
  `buildSrgbIcc` 有意的分歧——Kotlin 漏写标签签名（未使用的无效 profile），Rust 版补齐为有效 ICC
- ICC 默认注入 `assets/2020_profile.icc`（自动探测，可 `--icc` 覆盖；`--output-gamut` 可切 P3 / sRGB，
  这两者由 `build_pq_icc_for_gamut` 程序化生成 **PQ** profile——不能复用只有 sRGB TRC 的
  `display_p3_primary.icc` / `build_srgb_icc()`）；ultra-hdr 主/增益图 ICC 用内嵌的 Google 常量（Display-P3 / sRGB）
- GPU：`backend/cuda/jni/build_ffi.bat`（nvcc → `backend/cuda/hdr_gpu_ffi.dll`）导出
  `hdr_ffi_*` C ABI；打包时该 DLL 与 ffmpeg 一样放 asarUnpack；
  `src/gpu.rs` 经 libloading 加载，`HDRCONV_GPU=1` 显式启用（默认 CPU 保对齐）。
  **图片 ICC 链路（`_v2`）与增益图（`_bcurve`）已与 CPU 同式**：B 曲线（rolloff + image_gamma）+
  输出色域 + Oklab 色域拓展（查表由 Rust 预计算后上传，CPU/GPU 共用）+ 增益图 top-hat 局部高光；
  输入解读固定 sRGB/BT.709，非默认 `input_transfer`/`input_primaries` 自动回退 CPU；
  视频 16-bit 重建 GPU 仍可用（仅 `hdrconv video` CLI 链路接入）
- 视频链路（`reconstruct_linear_hdr_transform`、`video_direct_preview_rgba`）已作为库函数，
  并经 `hdrconv video` 端到端验证