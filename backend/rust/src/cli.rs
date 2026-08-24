use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::models::{OutputFormat, RgbAdjustment, Settings};

/// HDR 转换 CLI：HDR PNG / HDR JPEG（ICC 增益）/ Ultra HDR JPEG / 视频 HDR10
///
/// 参数集对齐 Electron 前端 settings（峰值亮度 / 白点 / 伽马 / RGB 通道），
/// 默认值见 `models.rs` 与 Models.kt 对照。
#[derive(Parser, Debug, Clone)]
#[command(name = "hdrconv", version, about, long_about = None)]
pub struct Cli {
    /// 输入图片路径（多个 = 批量转换；使用 `video` 子命令时可不填）
    #[arg()]
    pub inputs: Vec<String>,

    /// 输出路径（仅单输入生效；默认不指定时生成 `<输入名>.hdr.<ext>`）
    #[arg(short, long)]
    pub output: Option<String>,

    /// 输出格式：png | jpg | jpg-icc | ultra-hdr
    /// 注意：**jpg 即 Ultra HDR**（Kotlin `/convert` 的 "jpg" 语义 = 增益图链路）；
    /// jpg-icc = ICC 增益 JPEG（Rec.2020/PQ）
    #[arg(short = 'f', long, default_value = "png")]
    pub format: String,

    /// 峰值亮度（尼特），范围 400~1250，默认 574（= 203×2^1.5EV，与 Electron UI 一致）
    #[arg(long, default_value_t = 574.0)]
    pub peak: f64,

    /// 白点（SDR 参考白，尼特，BT.2408），默认 203
    #[arg(long, default_value_t = 203.0)]
    pub white_point: f64,

    /// 伽马，默认 0.9
    #[arg(long, default_value_t = 0.9)]
    pub gamma: f64,

    /// RGB 通道调整 `r,g,b`（仅「直接转」生效；默认 0.96,1.0,1.0）
    #[arg(long, value_parser = parse_rgb)]
    pub rgb: Option<(f64, f64, f64)>,

    /// JPEG 质量 0..1（默认 1.0 = 100%，仅 jpg 类输出生效）
    #[arg(long, default_value_t = 1.0)]
    pub quality: f64,

    /// 主图用原始 sRGB 像素 + sRGB ICC（实验开关，对应 Kotlin primarySrgb）
    #[arg(long)]
    pub primary_srgb: bool,

    /// ICC 配置文件路径（默认自动探测 assets/2020_profile.icc；png / jpg-icc 注入用）
    #[arg(long)]
    pub icc: Option<String>,

    /// 批量最大并发数（默认 = 核心数/2 + 1）
    #[arg(short = 'j', long)]
    pub jobs: Option<usize>,

    /// 只探测输入色彩空间并打印，不转换
    #[arg(long)]
    pub check: bool,

    /// 子命令（视频转换）
    #[command(subcommand)]
    pub cmd: Option<Command>,
}

/// 子命令。
#[derive(Subcommand, Debug, Clone)]
pub enum Command {
    /// 视频 → HDR10 MP4（逐帧重建；← video_converter.js convertVideoFrames）
    Video(VideoArgs),
}

/// `hdrconv video` 参数（← video_converter.js settings/opts）。
#[derive(Args, Debug, Clone)]
pub struct VideoArgs {
    /// 输入视频（SDR）
    #[arg(required = true)]
    pub input: String,

    /// 输出 MP4（默认 `<输入名>_hdr.mp4`）
    #[arg(short, long)]
    pub output: Option<String>,

    /// 重建模式：frames=逐帧增益图（默认）| direct=单层色调映射（图片 jpg_icc 式）
    #[arg(long, default_value = "frames")]
    pub mode: String,

    /// 峰值亮度（尼特），默认 1000（max-cll / npl / PAM 归一峰值）
    #[arg(long, default_value_t = 1000.0)]
    pub peak: f64,

    /// 白点（SDR 参考白，尼特，BT.2408），默认 203
    #[arg(long, default_value_t = 203.0)]
    pub white_point: f64,

    /// 伽马（高光掩膜曲线），默认 0.9
    #[arg(long, default_value_t = 0.9)]
    pub gamma: f64,

    /// 增益图 EV（仅 frames 模式；默认跟随峰值联动 log2(峰值/白点)，对应 JS hdrIntensity）
    #[arg(long)]
    pub hdr_intensity: Option<f64>,

    /// 编码质量 CRF，默认 20
    #[arg(long, default_value_t = 20)]
    pub crf: u32,

    /// 编码器：x265 | nvenc | av1 | av1-nvenc（默认 x265；不可用时自动降级回退）
    #[arg(long, default_value = "x265")]
    pub encoder: String,

    /// 处理宽度上限（>0 时缩放省内存），默认 0 = 原始分辨率
    #[arg(long)]
    pub max_width: Option<u32>,

    /// 帧处理并发（默认 = 核心数，上限 8）
    #[arg(short = 'j', long)]
    pub jobs: Option<usize>,

    /// ffmpeg.exe 路径（默认自动探测 backend/ffmpeg/）
    #[arg(long)]
    pub ffmpeg: Option<PathBuf>,

    /// ffprobe.exe 路径（默认自动探测 backend/ffmpeg/）
    #[arg(long)]
    pub ffprobe: Option<PathBuf>,
}

fn parse_rgb(s: &str) -> Result<(f64, f64, f64), String> {
    let parts: Vec<&str> = s.split(',').map(str::trim).collect();
    if parts.len() != 3 {
        return Err("RGB 需为 r,g,b 三个值，如 0.96,1.0,1.0".into());
    }
    let v = |i: usize| {
        parts[i]
            .parse::<f64>()
            .map_err(|_| format!("无法解析通道值 '{}'", parts[i]))
    };
    Ok((v(0)?, v(1)?, v(2)?))
}

/// 由 CLI 参数构造 Settings。
pub fn settings_from_cli(cli: &Cli) -> Settings {
    Settings {
        peak_nits: cli.peak,
        white_nits: cli.white_point,
        gamma: cli.gamma,
        rgb: cli
            .rgb
            .map(|(r, g, b)| RgbAdjustment { red: r, green: g, blue: b })
            .unwrap_or_default(),
        quality: cli.quality,
        primary_srgb: cli.primary_srgb,
        icc_path: cli.icc.clone(),
    }
}

/// 供测试等场景直接解析格式字符串。
pub fn parse_format(s: &str) -> Result<OutputFormat, String> {
    OutputFormat::parse(s)
}