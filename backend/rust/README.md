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
cargo test                        # 常规回归测试
cargo test -- --ignored           # Kotlin 逐像素对照（需先生成基准，见下）
```

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
| `src/gpu.rs` | `HdrGpuJni.kt` + `backend/cuda/include/hdr_gpu.h` | feature `gpu`（默认关闭），需确认 DLL 导出符号 |
| `tests/regression.rs` | — | 常规测试 + Kotlin 逐像素对照（`--ignored`） |

## 移植顺序建议

1. ✅ `convert.rs::apply_hdr_rec2020_pq`（/convert 的 png / jpg_icc 管线，已与 Kotlin 逐位对齐）
2. ✅ `icc.rs`（PNG iCCP / JPEG APP2 注入，逐位对齐 Kotlin；png / jpg-icc 已端到端可用）
3. ✅ `colorspace.rs::detect`（ICC 主色匹配 / EXIF ColorSpace / JFIF / PNG 标记，顺序与 Kotlin 一致）
4. ✅ `ultra_hdr.rs`（compute_gain_map → encode_ultra_hdr；XMP 数值与 Kotlin 完全一致，JPEG
   编码器不同 → 字节流不一致属预期；jpg = Ultra HDR 语义已对齐）

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
- 视频链路（`reconstruct_linear_hdr_frame/transform`、`video_direct_preview_rgba`）已移植为
  库函数，ffmpeg 封装与 /video-frame HTTP 服务为下一阶段