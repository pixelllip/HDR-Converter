# hdrconv —— Rust 版 HDR 转换 CLI

Electron 版 HDR 转换器的 Rust 后端（对应 Kotlin 后端 `backend/kotlin` 的 1:1 移植）。
CLI 形态，后续可演进为常驻 HTTP 服务（用 axum 替代 Ktor）供 Electron 调用。

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
# 视频转换用子命令：
cargo run -- video input.mp4 -o out.mp4 --mode frames --peak 1000
cargo run -- video input.mp4 --mode direct --encoder nvenc   # 单层色调映射 / 硬编
cargo test                        # 常规回归测试
cargo test -- --ignored           # Kotlin 逐像素对照（需先生成基准，见下）
```

## 视频链路（`hdrconv video`）

```
ffprobe 探测 → ffmpeg 拆 PNG 帧 → 逐帧 Rust 重建 16-bit PAM（帧级并发 ≤8）
→ 管道喂给 ffmpeg 编码器（pam_pipe + zscale 线性→2020/PQ + yuv420p10le）
→ 无声 HDR MP4 →（nvenc 时 libx265 归一 coded 补边）→ 合音频 → 注入 mdcv/clli 盒
```

- `--mode frames`（默认）= 逐帧增益图（对应 JS 增益图链路 / Kotlin mode=gainmap）；
  `--mode direct` = 单层色调映射（jpg_icc 式 / mode=transform）
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
| `src/convert.rs` | `HdrConverter.kt` | **Rec.2020/PQ 已移植**（png/jpg_icc 管线，逐位对齐）；legacy `applyHdrTransform` 待接线 |
| `src/ultra_hdr.rs` | `UltraHdrEncoder.kt` | **已移植**（增益图/双 JPEG/XMP/MPF/ICC 组装 + 视频帧重建 + 自动估算/下采样） |
| `src/icc.rs` | `IccInjector.kt` | **已移植**（PNG iCCP / JPEG APP2，逐位对齐） |
| `src/colorspace.rs` | `ColorSpaceDetector.kt` | **已移植**（ICC 主色匹配 / EXIF / JFIF / PNG 标记） |
| `src/gpu.rs` | `HdrGpuJni.kt` + `backend/cuda/include/hdr_gpu.h` | FFI 就绪（feature `gpu`）；**DLL 实证仅导出 JNI**（`examples/dump_exports.rs` 枚举），启用需 CUDA 侧补 C-ABI 导出 |
| `src/video.rs` | `video_converter.js`（convertVideoFrames）+ `mp4_hdr.js` | **已移植**（探测/NVDEC 硬解/拆帧/逐帧重建/编码器降级/mdcv+clli 注入/Eclipsa） |
| `src/server.rs` | `Main.kt`（Ktor 路由） | **已移植**（axum 1:1 端点契约；`hdrconv serve`） |
| `src/st2094_50.rs` + `src/eclipsa.rs` | `st2094_50.js` + `st2094_50_inject.js` + `hevc_inject.js` | **已移植**（ST 2094-50 载荷/SEI 注入） |
| `tests/regression.rs` | — | 常规测试 + Kotlin 逐像素对照（`--ignored`） |

## 移植顺序建议

1. ✅ `convert.rs::apply_hdr_rec2020_pq`（/convert 的 png / jpg_icc 管线，已与 Kotlin 逐位对齐）
2. ✅ `icc.rs`（PNG iCCP / JPEG APP2 注入，逐位对齐 Kotlin；png / jpg-icc 已端到端可用）
3. ✅ `colorspace.rs::detect`（ICC 主色匹配 / EXIF ColorSpace / JFIF / PNG 标记，顺序与 Kotlin 一致）
4. ✅ `ultra_hdr.rs`（compute_gain_map → encode_ultra_hdr；XMP 数值与 Kotlin 完全一致，JPEG
   编码器不同 → 字节流不一致属预期；jpg = Ultra HDR 语义已对齐）
5. ✅ `video.rs`（视频 → HDR10：ffprobe/NVDEC/拆帧/逐帧重建/编码/合流/mdcv+clli；验证
   `yuv420p10le,smpte2084,bt2020` + 首帧线性峰值 ≈ 峰值亮度）
6. ✅ `server.rs`（axum HTTP 服务，1:1 复刻 Kotlin 端点契约；`hdrconv serve` → `HDR_BACKEND_PORT:<port>`）
7. ✅ 视频 NVDEC 硬解（cuvid 支持集 + 失败回退 CPU）
8. ✅ Eclipsa（`--eclipsa`：signalstats 逐窗 MaxCLL → 参考白配方载荷 → 按 AUD 注入 Prefix_SEI）

## 待办（后续阶段）

- **CUDA 侧（需要 nvcc 工具链，本机已装 13.2）**：让 `backend/cuda` 额外导出
  `hdr_gpu_*` C ABI（定义 `HDR_GPU_EXPORTS`），并可按 `HdrGpuJni.kt` native 方法扩展
  Rec.2020/PQ、增益图、16-bit 重建的 C ABI 变体——之后 `cargo build --features gpu` 即可启用
  （当前 DLL 仅导出 JNI 符号，见 `src/gpu.rs` 说明）
- Electron 主进程切换：`main.js` 把 `java -jar` 换成 `hdrconv serve`（保留 Kotlin 回退开关）
- 性能基准：`tests/tmp_bench*.{rs,js}`（gitignored）

## 注意事项

- 参数语义对齐：总曝光 = 峰值/白点（2^EV），默认峰值 574、白点 203（Electron UI 默认；
  Kotlin 后端回退默认是 1000/203，调用时务必显式传参）
- 格式语义已对齐 Kotlin：`jpg` = Ultra HDR（增益图链路），`jpg-icc` = ICC 增益（Rec.2020/PQ），
  `png` = HDR PNG（Rec.2020/PQ + 2020 ICC）
- **对齐粒度说明**：png 与 Kotlin 逐像素一致；ultra-hdr 的增益图数学与 XMP 数值 +1e-9 内一致
  （JPEG 编码器不同，JPEG 字节流不可能逐位一致）；`buildSrgbIcc` 有意的分歧——Kotlin 漏写
  标签签名（未使用的无效 profile），Rust 版补齐为有效 ICC
- ICC 默认注入 `assets/2020_profile.icc`（自动探测，可 `--icc` 覆盖）；ultra-hdr 主/增益图 ICC
  用内嵌的 Google 常量（Display-P3 / sRGB）
- GPU DLL（`backend/cuda/hdr_gpu_jni.dll`）打包后位于 asarUnpack，与主进程 JAR 处理一致
- 视频链路（`reconstruct_linear_hdr_frame/transform`、`video_direct_preview_rgba`）已作为库函数，
  并经 `hdrconv video` 端到端验证