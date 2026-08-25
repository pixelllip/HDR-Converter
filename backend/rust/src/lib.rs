//! hdrconv —— Rust 版 HDR 转换 CLI（库根）。
//!
//! 模块划分与 Kotlin 后端 `backend/kotlin/src/main/kotlin/com/hdrconverter/` 一一对应，
//! 每个模块头部的 `←` 注释标注了对应 Kotlin 源文件，移植时按文件对号入座：
//!
//! | Rust 模块        | Kotlin 源文件                          | 状态           |
//! |------------------|----------------------------------------|----------------|
//! | `cli`            | Electron 前端 settings（无对应文件）   | 已接线（clap） |
//! | `models`         | Models.kt（ConversionSettings 等）     | 已接线         |
//! | `convert`        | HdrConverter.kt（变换核心 + 图像IO）   | Rec.2020/PQ 已移植（png/jpg_icc 管线）；legacy 待接线 |
//! | `ultra_hdr`      | UltraHdrEncoder.kt（编码器+重建）      | 已移植（增益图/双 JPEG/XMP/MPF/ICC + 视频帧重建） |
//! | `icc`            | IccInjector.kt（ICC 注入）             | 已移植（PNG iCCP / JPEG APP2） |
//! | `colorspace`     | ColorSpaceDetector.kt（探测）          | 已移植（ICC/EXIF/JFIF/PNG） |
//! | `gpu`            | HdrGpuJni.kt + backend/cuda/hdr_gpu.h  | FFI 就绪（feature `gpu`）；DLL 仅导出 JNI，启用需 CUDA 侧补 C-ABI 导出 |
//! | `video`          | video_converter.js + mp4_hdr.js        | 已移植（逐帧重建/编码管道/mdcv+clli 注入） |

pub mod cli;
pub mod colorspace;
pub mod convert;
pub mod eclipsa;
pub mod gpu;
pub mod icc;
pub mod models;
pub mod server;
pub mod st2094_50;
pub mod ultra_hdr;
pub mod video;

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use rayon::prelude::*;

use models::{OutputFormat, Settings};

/// 单文件转换结果。
#[derive(Debug)]
pub struct ConvertOutcome {
    pub width: u32,
    pub height: u32,
    pub detected_space: colorspace::InputColorSpace,
}

/// 单文件转换管线。
///
/// 步骤对应 Kotlin `Main.kt` 的 /convert 处理（`encodeAndInjectIcc`，Main.kt:585）：
/// 1. 读图（← `HdrConverter.readImageAsRgba`，HdrConverter.kt:235）
/// 2. 色彩空间探测（← `ColorSpaceDetector.detect`，ColorSpaceDetector.kt:215）
/// 3. 变换：png / jpg_icc 走 Rec.2020/PQ 管线
///    （← `HdrConverter.applyHdrTransformToRec2020Pq`，HdrConverter.kt:149；
///    jpg=UltraHDR 对应 `UltraHdrEncoder.encode`，UltraHdrEncoder.kt:954）
/// 4. 编码输出 + ICC 注入：png / jpg-icc 走 Rec.2020/PQ + ICC（对齐 Kotlin
///    `encodeAndInjectIcc`）；jpg / ultra-hdr 走增益图链路（`UltraHdrEncoder.encode`，
///    主图 = 原始输入像素，非变换后像素）
///
/// 当前实现：四种格式全部可用 — png / jpg-icc 像素与 Kotlin 逐位对齐；
/// ultra-hdr 结构对齐（JPEG 编码器不同 → 字节流不一致，但增益图数学/XMP 数值一致）。
pub fn convert_image(
    input: &Path,
    output: &Path,
    settings: &Settings,
    format: OutputFormat,
) -> Result<ConvertOutcome> {
    // 1) 读图
    let img = convert::read_image_rgba(input)?;

    // 2) 探测输入色彩空间（← ColorSpaceDetector.detect；ICC > EXIF > JFIF/PNG > UNKNOWN）
    let detected_space = colorspace::detect(input);

    // 3+4) 变换 + 编码 + ICC（与 Kotlin encodeAndInjectIcc 行为一致）
    let bytes = encode_image_bytes(&img, settings, format, Some(detected_space))?;
    std::fs::write(output, bytes)
        .with_context(|| format!("写入输出失败: {}", output.display()))?;

    Ok(ConvertOutcome {
        width: img.width,
        height: img.height,
        detected_space,
    })
}

/// 编码为输出字节（供 /convert 写盘与 /preview dataUrl 复用）。
///
/// png / jpg-icc：Rec.2020/PQ 变换 + ICC 注入（与 Kotlin `encodeAndInjectIcc` 一致）；
/// jpg / ultra-hdr：增益图链路（主图 = 原始输入像素）。
/// @param detected 输入色彩空间（预览场景可传 None → 按未声明处理）。
pub fn encode_image_bytes(
    img: &convert::ImageData,
    settings: &Settings,
    format: OutputFormat,
    detected: Option<colorspace::InputColorSpace>,
) -> Result<Vec<u8>> {
    let detected_space = detected.unwrap_or(colorspace::InputColorSpace::Unknown);
    let icc = match format {
        OutputFormat::Png | OutputFormat::JpgIcc => Some(resolve_icc(settings)?),
        _ => None,
    };
    let transformed = match gpu::try_gpu_rec2020_pq(img, settings) {
        Some(px) => convert::ImageData {
            pixels: px,
            width: img.width,
            height: img.height,
        },
        None => convert::apply_hdr_rec2020_pq(img, settings)?,
    };
    match format {
        OutputFormat::Png => {
            let bytes = convert::encode_png_bytes(&transformed)?;
            icc::inject_icc_into_png(&bytes, icc.as_deref().unwrap())
        }
        OutputFormat::Jpg => convert::encode_jpeg_bytes(&transformed, settings.quality),
        OutputFormat::JpgIcc => {
            let bytes = convert::encode_jpeg_bytes(&transformed, settings.quality)?;
            icc::inject_icc_into_jpeg(&bytes, icc.as_deref().unwrap())
        }
        OutputFormat::UltraHdr => ultra_hdr::encode_ultra_hdr(
            &img.pixels,
            img.width,
            img.height,
            settings,
            Some(detected_space),
        ),
    }
}

/// CLI 入口（由 `src/main.rs` 调用）。
pub fn run(cli: cli::Cli) -> Result<()> {
    // 子命令：HTTP 服务（Electron 引擎切换入口；与 Kotlin JAR 相同的端口行协议）
    if let Some(cli::Command::Serve(s)) = &cli.cmd {
        let host = s.host.clone();
        let port = s.port;
        let rt = tokio::runtime::Runtime::new().context("创建 tokio runtime 失败")?;
        return rt.block_on(server::serve(&host, port));
    }
    // 子命令：视频转换
    if let Some(cli::Command::Video(v)) = &cli.cmd {
        let input = PathBuf::from(&v.input);
        let output = v
            .output
            .as_deref()
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                let mut s = video::pos_last_dot(&input);
                s.push_str("_hdr.mp4");
                PathBuf::from(s)
            });
        let mode = match v.mode.as_str() {
            "direct" | "transform" => video::TransformMode::Transform,
            _ => video::TransformMode::Gainmap,
        };
        let opts = video::VideoOptions {
            mode,
            peak_nits: v.peak,
            white_nits: v.white_point,
            gamma: v.gamma,
            hdr_intensity: v.hdr_intensity,
            crf: v.crf,
            encoder: v.encoder.clone(),
            max_width: v.max_width,
            jobs: v.jobs,
            ffmpeg: v.ffmpeg.clone(),
            ffprobe: v.ffprobe.clone(),
            eclipsa: v.eclipsa,
            eclipsa_scheme: v.eclipsa_scheme.clone(),
            eclipsa_windows: v.eclipsa_windows,
        };
        let out = video::run_video(&input, &output, &opts)?;
        println!(
            "✓ {} -> {}（{} 帧, {}x{}, 编码器 {}）",
            input.display(),
            output.display(),
            out.frames,
            out.width,
            out.height,
            out.encoder_used
        );
        return Ok(());
    }

    // 子命令：Eclipsa 后处理（对已完成 HDR10 MP4 附加 ST 2094-50 动态元数据）
    // 路径 1：文件级后处理与引擎解耦，Electron 主进程在视频转换收尾时 spawn 本子命令。
    if let Some(cli::Command::AttachEclipsa(a)) = &cli.cmd {
        let input = PathBuf::from(&a.input);
        let output = a
            .output
            .as_deref()
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                let mut s = video::pos_last_dot(&input);
                s.push_str("_eclipsa.mp4");
                PathBuf::from(s)
            });
        let ffmpeg = video::find_tool(a.ffmpeg.as_deref(), "ffmpeg")?;
        let ffprobe = video::find_tool(a.ffprobe.as_deref(), "ffprobe")?;
        let opts = eclipsa::EclipsaOptions {
            ref_white_nits: a.ref_white,
            max_cll: a.max_cll,
            max_fall: a.max_fall,
            scheme: if a.scheme == "uniform" {
                eclipsa::WindowScheme::Uniform
            } else {
                eclipsa::WindowScheme::Scene
            },
            uniform_windows: a.windows.max(1),
            scene_threshold: a.scene_threshold,
            min_window_sec: a.min_window_sec,
            ffmpeg,
            ffprobe,
        };
        let out = eclipsa::attach_eclipsa(&input, &output, &opts)?;
        println!(
            "✓ {} -> {}（{} 窗 / {} 条 ST 2094-50 SEI）",
            input.display(),
            output.display(),
            out.windows.len(),
            out.total_sei
        );
        return Ok(());
    }

    // 图片转换入口校验
    if cli.inputs.is_empty() {
        return Err(anyhow!(
            "缺少输入图片路径（图片：hdrconv <图片...>；视频：hdrconv video <视频> -o out.mp4）"
        ));
    }

    let format = OutputFormat::parse(&cli.format).map_err(|e| anyhow!(e))?;

    // --check：只探测输入色彩空间
    if cli.check {
        for input in &cli.inputs {
            let space = colorspace::detect(Path::new(input));
            println!("{input}: 色彩空间 = {space}");
        }
        return Ok(());
    }

    let settings = cli::settings_from_cli(&cli);

    // 默认并发 = 核心数/2 + 1（与 Electron 批量转换一致）
    let jobs = cli.jobs.unwrap_or_else(|| {
        std::thread::available_parallelism()
            .map(|n| (n.get() / 2 + 1).max(1))
            .unwrap_or(2)
    });

    let tasks: Vec<(PathBuf, PathBuf)> = cli
        .inputs
        .iter()
        .map(|i| {
            let input = PathBuf::from(i);
            let output = derive_output(&input, cli.output.as_deref(), format);
            (input, output)
        })
        .collect();

    let convert_one = |(input, output): &(PathBuf, PathBuf)| -> Result<()> {
        match convert_image(input, output, &settings, format) {
            Ok(o) => {
                println!(
                    "✓ {} -> {} ({}x{}, 探测={})",
                    input.display(),
                    output.display(),
                    o.width,
                    o.height,
                    o.detected_space
                );
                Ok(())
            }
            Err(e) => {
                eprintln!("✗ {}: {e:#}", input.display());
                Err(e)
            }
        }
    };

    let fails;
    if tasks.len() > 1 && jobs > 1 {
        let pool = rayon::ThreadPoolBuilder::new().num_threads(jobs).build()?;
        let results: Vec<Result<()>> = pool.install(|| tasks.par_iter().map(convert_one).collect());
        fails = results.iter().filter(|r| r.is_err()).count();
    } else {
        fails = tasks.iter().filter(|t| convert_one(t).is_err()).count();
    }

    if fails > 0 {
        eprintln!(
            "完成：{}/{} 成功，{fails} 失败",
            tasks.len() - fails,
            tasks.len()
        );
    }
    Ok(())
}

/// 输出路径推导：显式指定（仅单输入）或 `<输入名>.hdr.<格式扩展名>`。
fn derive_output(input: &Path, explicit: Option<&str>, format: OutputFormat) -> PathBuf {
    if let Some(out) = explicit {
        return PathBuf::from(out);
    }
    let mut name = input.file_stem().unwrap_or_default().to_os_string();
    name.push(".hdr.");
    name.push(format.ext());
    input.with_file_name(name)
}

/// 解析 ICC 配置文件：显式 --icc 优先，否则自动探测（对齐 Kotlin `resolveIccProfilePath` 思路）。
pub(crate) fn resolve_icc(settings: &Settings) -> Result<Vec<u8>> {
    let path = settings
        .icc_path
        .as_deref()
        .map(PathBuf::from)
        .or_else(icc::find_default_icc)
        .ok_or_else(|| {
            anyhow!("找不到 ICC 配置文件（默认 assets/2020_profile.icc），可用 --icc 指定")
        })?;
    icc::read_icc_profile(&path)
}