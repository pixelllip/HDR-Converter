//! 视频 → HDR10 链路（← video_converter.js 的 convertVideoFrames + mp4_hdr.js）。
//!
//! 流程（与 JS 端一致）：
//!   ffprobe 探测 → ffmpeg 拆 PNG 帧 → 逐帧 Rust 重建 16-bit PAM（帧级并发）
//!   → 按序管道喂给 ffmpeg 编码器（pam_pipe + zscale 线性→2020/PQ + 10-bit）
//!   → 无声 HDR MP4 →（nvenc 时 libx265 归一 coded 补边）→ 合音频
//!   → 注入 mdcv / clli 容器盒（← mp4_hdr.js）→ 清理
//!
//! 与 JS 端差异（有意）：
//!   - 帧级重建直接调用 Rust 库函数（不再走 Kotlin HTTP /video-frame）
//!   - 解码只用 CPU 软解（JS 尝试 CUDA NVDEC + 回退；CLI v1 从简，后续可加）
//!   - 默认帧处理并发 = 核心数（上限 8），与 JS FRAME_CONCURRENCY 一致
//!   - Eclipsa（ST 2094-50 动态元数据）暂未移植（JS st2094_50_inject.js）

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, bail, Context, Result};

use crate::models::Settings;
use crate::ultra_hdr;

// 默认白点 / 峰值（与 video_converter.js 一致）
const DEFAULT_WHITE_NITS: f64 = 203.0;
const DEFAULT_PEAK_NITS: f64 = 1000.0;
/// P3 主色 mastering display（与 -x265-params master-display 完全一致）
const MASTER_DISPLAY: &str =
    "master-display=G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)L(10000000,1)";

/// 重建模式：frames=逐帧增益图（默认），direct=单层色调映射（图片 jpg_icc 式）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransformMode {
    Gainmap,
    Transform,
}

/// 视频转换参数（← video_converter.js convertVideoFrames 的 settings/opts）。
#[derive(Debug, Clone)]
pub struct VideoOptions {
    pub mode: TransformMode,
    pub peak_nits: f64,
    pub white_nits: f64,
    pub gamma: f64,
    /// 增益图 EV（None = 峰值联动 log2(峰值/白点)，对应 JS settings.hdrIntensity）
    pub hdr_intensity: Option<f64>,
    pub crf: u32,
    /// x265 | nvenc | av1 | av1-nvenc（默认 x265；不可用时按降级链回退）
    pub encoder: String,
    /// 处理宽度上限（None=原始分辨率）
    pub max_width: Option<u32>,
    /// 帧处理并发（None=核心数，上限 8）
    pub jobs: Option<usize>,
    pub ffmpeg: Option<PathBuf>,
    pub ffprobe: Option<PathBuf>,
    /// Eclipsa（ST 2094-50 动态元数据）附加开关
    pub eclipsa: bool,
    /// Eclipsa 窗口方案：scene（镜头切，默认）| uniform
    pub eclipsa_scheme: String,
    /// Eclipsa uniform 窗口数（默认 3）
    pub eclipsa_windows: usize,
}

impl Default for VideoOptions {
    fn default() -> Self {
        Self {
            mode: TransformMode::Gainmap,
            peak_nits: DEFAULT_PEAK_NITS,
            white_nits: DEFAULT_WHITE_NITS,
            gamma: 0.9,
            hdr_intensity: None,
            crf: 20,
            encoder: "x265".into(),
            max_width: None,
            jobs: None,
            ffmpeg: None,
            ffprobe: None,
            eclipsa: false,
            eclipsa_scheme: "scene".into(),
            eclipsa_windows: 3,
        }
    }
}

/// 转换结果。
#[derive(Debug)]
pub struct VideoOutcome {
    pub encoder_used: String,
    pub width: u32,
    pub height: u32,
    pub frames: usize,
    pub fps: f64,
}

// ============================================================
//  工具：ffmpeg/ffprobe 定位与运行
// ============================================================

fn find_tool(explicit: Option<&Path>, name: &str) -> Result<PathBuf> {
    if let Some(p) = explicit {
        if !p.exists() {
            bail!("指定工具不存在: {}", p.display());
        }
        return Ok(p.to_path_buf());
    }
    // 自动探测：exe 同目录 / cwd / 仓库相对路径（对齐主进程 resourcePath 候选思路）
    let candidates = [
        format!("backend/ffmpeg/{name}.exe"),
        format!("../backend/ffmpeg/{name}.exe"),
        format!("../../backend/ffmpeg/{name}.exe"),
        format!("../../../backend/ffmpeg/{name}.exe"),
        name.to_string(),
    ];
    for c in candidates {
        let p = PathBuf::from(&c);
        if p.exists() {
            return Ok(p);
        }
    }
    bail!("找不到 {name}.exe，请用 --{name} 显式指定（默认 backend/ffmpeg/）")
}

/// 运行命令并收集输出；code==0 返回 (stdout, stderr)，否则报错（带 stderr 尾部）。
fn run_capture(cmd: &Path, args: &[&str]) -> Result<(String, String)> {
    let out = Command::new(cmd)
        .args(args)
        .output()
        .with_context(|| format!("运行失败: {} {}", cmd.display(), args.join(" ")))?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    if !out.status.success() {
        bail!(
            "{} 退出码 {}: {}",
            cmd.display(),
            out.status.code().unwrap_or(-1),
            stderr.chars().rev().take(600).collect::<String>().chars().rev().collect::<String>()
        );
    }
    Ok((stdout, stderr))
}

// ============================================================
//  探测（← probeVideo / probeCodedHeight）
// ============================================================

#[derive(Debug, Default)]
struct ProbeInfo {
    width: u32,
    height: u32,
    duration: f64,
    fps: f64,
    frames: usize,
    codec: String,
    has_audio: bool,
    coded_height: u32,
}

fn parse_ratio(s: &str) -> Option<f64> {
    let parts: Vec<&str> = s.split('/').collect();
    if parts.len() == 2 {
        let a: f64 = parts[0].parse().ok()?;
        let b: f64 = parts[1].parse().ok()?;
        if b != 0.0 {
            return Some(a / b);
        }
    }
    s.parse().ok()
}

fn probe_video(ffprobe: &Path, input: &Path) -> Result<ProbeInfo> {
    let (out, _) = run_capture(
        ffprobe,
        &[
            "-v", "error", "-of", "json", "-show_streams", "-show_format",
            input.to_str().unwrap_or(""),
        ],
    )?;
    let j: serde_json::Value = serde_json::from_str(&out).context("ffprobe JSON 解析失败")?;
    let mut info = ProbeInfo::default();
    let mut video: Option<&serde_json::Value> = None;
    if let Some(streams) = j.get("streams").and_then(|s| s.as_array()) {
        for s in streams {
            let kind = s.get("codec_type").and_then(|v| v.as_str()).unwrap_or("");
            if kind == "video" && video.is_none() {
                video = Some(s);
            }
            if kind == "audio" {
                info.has_audio = true;
            }
        }
    }
    let vs = video.ok_or_else(|| anyhow!("输入文件中没有视频流"))?;
    info.width = vs.get("width").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    info.height = vs.get("height").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    info.coded_height = vs
        .get("coded_height")
        .and_then(|v| v.as_u64())
        .unwrap_or(info.height as u64) as u32;
    info.codec = vs.get("codec_name").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let dur = vs
        .get("duration")
        .and_then(|v| v.as_str())
        .and_then(parse_ratio)
        .or_else(|| {
            j.get("format")
                .and_then(|f| f.get("duration"))
                .and_then(|v| v.as_str())
                .and_then(parse_ratio)
        })
        .unwrap_or(0.0);
    info.duration = dur;
    let rate = vs
        .get("avg_frame_rate")
        .and_then(|v| v.as_str())
        .and_then(parse_ratio)
        .or_else(|| vs.get("r_frame_rate").and_then(|v| v.as_str()).and_then(parse_ratio))
        .unwrap_or(30.0);
    info.fps = rate;
    info.frames = (dur * rate).round() as usize;
    Ok(info)
}

// ============================================================
//  编码器选择（← buildEncoderArgs / encoderAvailable / 降级链）
// ============================================================

fn build_encoder_args(encoder: &str, crf: u32, x265_params: &str) -> Vec<String> {
    match encoder {
        "nvenc" => vec![
            "-c:v".into(), "hevc_nvenc".into(), "-preset".into(), "p5".into(),
            "-rc".into(), "vbr".into(), "-cq".into(), crf.to_string(),
            "-b:v".into(), "0".into(), "-tag:v".into(), "hvc1".into(),
        ],
        "av1_nvenc" => vec![
            "-c:v".into(), "av1_nvenc".into(), "-preset".into(), "p5".into(),
            "-rc".into(), "vbr".into(), "-cq".into(), crf.to_string(),
            "-b:v".into(), "0".into(), "-tag:v".into(), "av01".into(),
        ],
        "av1" => vec![
            "-c:v".into(), "libaom-av1".into(), "-crf".into(), crf.to_string(),
            "-b:v".into(), "0".into(), "-cpu-used".into(), "5".into(),
            "-row-mt".into(), "1".into(), "-tag:v".into(), "av01".into(),
        ],
        _ => vec![
            "-c:v".into(), "libx265".into(), "-preset".into(), "medium".into(),
            "-crf".into(), crf.to_string(), "-tag:v".into(), "hvc1".into(),
            "-x265-params".into(), x265_params.into(),
        ],
    }
}

/// 探测 ffmpeg 是否可用指定编码器（列表 + 实际试编码一帧，← encoderAvailable）。
fn encoder_available(ffmpeg: &Path, enc_name: &str) -> bool {
    let (out, _) = match run_capture(ffmpeg, &["-hide_banner", "-encoders"]) {
        Ok(o) => o,
        Err(_) => return false,
    };
    if !out.contains(enc_name) {
        return false;
    }
    let probe = Command::new(ffmpeg)
        .args([
            "-hide_banner", "-loglevel", "error",
            "-f", "lavfi", "-i", "color=black:s=320x240:d=0.04,format=yuv420p10le",
            "-frames:v", "1", "-c:v", enc_name, "-f", "null", "-",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    matches!(probe, Ok(st) if st.success())
}

fn pick_encoder(ffmpeg: &Path, requested: &str, crf: u32, x265_params: &str) -> (String, Vec<String>) {
    // 降级链：x265；nvenc→x265；av1_nvenc→av1→x265；av1→x265（与 JS fallbackChain 一致）
    let chain: Vec<&str> = match requested {
        "nvenc" => vec!["nvenc", "x265"],
        "av1_nvenc" => vec!["av1_nvenc", "av1", "x265"],
        "av1" => vec!["av1", "x265"],
        "x265" => vec!["x265"],
        other => return (other.to_string(), build_encoder_args(other, crf, x265_params)),
    };
    for name in chain {
        let probe_name = match name {
            "nvenc" => "hevc_nvenc",
            "av1_nvenc" => "av1_nvenc",
            "av1" => "libaom-av1",
            _ => "",
        };
        if name == "x265" || encoder_available(ffmpeg, probe_name) {
            return (name.to_string(), build_encoder_args(name, crf, x265_params));
        }
    }
    // 兜底 x265
    ("x265".to_string(), build_encoder_args("x265", crf, x265_params))
}

// ============================================================
//  mdcv / clli 容器盒注入（← mp4_hdr.js 逐位移植）
// ============================================================

#[derive(Debug, Clone, Copy)]
struct Mastering {
    gx: u16, gy: u16,
    bx: u16, by: u16,
    rx: u16, ry: u16,
    wx: u16, wy: u16,
    max_lum: u32,
    min_lum: u32,
}

const DEFAULT_MASTERING: Mastering = Mastering {
    gx: 13250, gy: 34500,
    bx: 7500, by: 3000,
    rx: 34000, ry: 16000,
    wx: 15635, wy: 16450,
    max_lum: 10_000_000,
    min_lum: 1,
};

fn read_box_size(buf: &[u8], off: usize) -> u64 {
    let s = u32::from_be_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]]) as u64;
    if s == 1 {
        let mut v = [0u8; 8];
        v.copy_from_slice(&buf[off + 8..off + 16]);
        u64::from_be_bytes(v)
    } else {
        s
    }
}

fn box_header_size(buf: &[u8], off: usize) -> usize {
    if u32::from_be_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]]) == 1 {
        16
    } else {
        8
    }
}

fn add_box_size(buf: &mut [u8], off: usize, delta: i64) {
    let s = u32::from_be_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]]) as u64;
    if s == 1 {
        let mut v = [0u8; 8];
        v.copy_from_slice(&buf[off + 8..off + 16]);
        let cur = u64::from_be_bytes(v) as i64;
        buf[off + 8..off + 16].copy_from_slice(&(cur + delta).to_be_bytes());
    } else if s != 0 {
        buf[off..off + 4].copy_from_slice(&((s as i64 + delta) as u32).to_be_bytes());
    }
}

fn build_mdcv(m: &Mastering) -> Vec<u8> {
    let mut b = vec![0u8; 32];
    b[0..4].copy_from_slice(&32u32.to_be_bytes());
    b[4..8].copy_from_slice(b"mdcv");
    b[8..10].copy_from_slice(&m.gx.to_be_bytes());
    b[10..12].copy_from_slice(&m.gy.to_be_bytes());
    b[12..14].copy_from_slice(&m.bx.to_be_bytes());
    b[14..16].copy_from_slice(&m.by.to_be_bytes());
    b[16..18].copy_from_slice(&m.rx.to_be_bytes());
    b[18..20].copy_from_slice(&m.ry.to_be_bytes());
    b[20..22].copy_from_slice(&m.wx.to_be_bytes());
    b[22..24].copy_from_slice(&m.wy.to_be_bytes());
    b[24..28].copy_from_slice(&m.max_lum.to_be_bytes());
    b[28..32].copy_from_slice(&m.min_lum.to_be_bytes());
    b
}

fn build_clli(max_cll: u16, max_fall: u16) -> Vec<u8> {
    let mut b = vec![0u8; 12];
    b[0..4].copy_from_slice(&12u32.to_be_bytes());
    b[4..8].copy_from_slice(b"clli");
    b[8..10].copy_from_slice(&max_cll.to_be_bytes());
    b[10..12].copy_from_slice(&max_fall.to_be_bytes());
    b
}

/// 在视频采样描述中定位插入点（← locateInsertion）。
fn locate_insertion(buf: &[u8], moov_start: usize, moov_end: usize) -> Option<(Vec<usize>, usize)> {
    const CONTAINERS: [&[u8; 4]; 6] = [b"moov", b"trak", b"mdia", b"minf", b"stbl", b"stsd"];
    let mut chain: Vec<usize> = Vec::new();
    fn walk(
        buf: &[u8],
        start: usize,
        end: usize,
        is_stsd: bool,
        chain: &mut Vec<usize>,
    ) -> Option<(Vec<usize>, usize)> {
        let mut off = start;
        while off + 8 <= end {
            let size = read_box_size(buf, off) as usize;
            let typ = [buf[off + 4], buf[off + 5], buf[off + 6], buf[off + 7]];
            let d_start = off + box_header_size(buf, off);
            let d_end = off + size;
            if is_stsd {
                if [*b"hvc1", *b"hev1", *b"avc1", *b"av01"].contains(&typ) {
                    // 视觉采样条目有 78 字节固定头，子盒从 dataStart+78 开始
                    let mut anchor: Option<usize> = None;
                    let mut e = d_start + 78;
                    while e + 8 <= d_end {
                        let esz = read_box_size(buf, e) as usize;
                        let etype = [buf[e + 4], buf[e + 5], buf[e + 6], buf[e + 7]];
                        if etype == *b"colr" {
                            anchor = Some(e);
                        }
                        if anchor.is_none() && (*b"hvcC" == etype || *b"avcC" == etype || *b"av1C" == etype) {
                            anchor = Some(e);
                        }
                        if esz == 0 {
                            break;
                        }
                        e += esz;
                    }
                    let a = anchor?;
                    let mut out = chain.clone();
                    out.push(off);
                    return Some((out, a + read_box_size(buf, a) as usize));
                }
            } else if CONTAINERS.contains(&&typ) {
                chain.push(off);
                let child_start = if typ == *b"stsd" || typ == *b"dref" {
                    d_start + 8
                } else {
                    d_start
                };
                if let Some(r) = walk(buf, child_start, d_end, typ == *b"stsd", chain) {
                    return Some(r);
                }
                chain.pop();
            }
            off = d_end;
            if size == 0 {
                break;
            }
        }
        None
    }
    walk(buf, moov_start, moov_end, false, &mut chain)
}

/// 递归查找所有 stco / co64 并调整块偏移（moov 在 mdat 之前时使用；← adjustChunkOffsets）。
fn adjust_chunk_offsets(buf: &mut [u8], moov_start: usize, moov_end: usize, delta: i64) {
    fn walk(buf: &mut [u8], start: usize, end: usize, delta: i64) {
        let mut off = start;
        while off + 8 <= end {
            let size = read_box_size(buf, off) as usize;
            let typ = [buf[off + 4], buf[off + 5], buf[off + 6], buf[off + 7]];
            let d_start = off + box_header_size(buf, off);
            let d_end = off + size;
            if typ == *b"stco" || typ == *b"co64" {
                // fullbox：version/flags(4B) + entry_count(4B) + entries
                let count =
                    u32::from_be_bytes([buf[d_start + 4], buf[d_start + 5], buf[d_start + 6], buf[d_start + 7]]) as usize;
                let mut p = d_start + 8;
                for _ in 0..count {
                    if typ == *b"stco" {
                        let v = u32::from_be_bytes([buf[p], buf[p + 1], buf[p + 2], buf[p + 3]]) as i64;
                        buf[p..p + 4].copy_from_slice(&((v + delta) as u32).to_be_bytes());
                        p += 4;
                    } else {
                        let mut v = [0u8; 8];
                        v.copy_from_slice(&buf[p..p + 8]);
                        let v = u64::from_be_bytes(v) as i64;
                        buf[p..p + 8].copy_from_slice(&((v + delta) as u64).to_be_bytes());
                        p += 8;
                    }
                }
            } else if [*b"moov", *b"trak", *b"mdia", *b"minf", *b"stbl"].contains(&typ) {
                walk(buf, d_start, d_end, delta);
            }
            off = d_end;
            if size == 0 {
                break;
            }
        }
    }
    walk(buf, moov_start, moov_end, delta);
}

/// 向 MP4 文件注入 mdcv / clli 盒（就地改写；← injectHdrBoxes）。
pub(crate) fn inject_hdr_boxes(path: &Path, max_cll: u16, max_fall: u16) -> Result<()> {
    let buf = std::fs::read(path).with_context(|| format!("读取 MP4 失败: {}", path.display()))?;
    if buf.len() < 16 {
        bail!("MP4 文件过小");
    }
    // 顶层盒
    let mut moov: Option<(usize, usize)> = None;
    let mut mdat: Option<(usize, usize)> = None;
    let mut off = 0usize;
    while off + 8 <= buf.len() {
        let size = read_box_size(&buf, off) as usize;
        let typ = [buf[off + 4], buf[off + 5], buf[off + 6], buf[off + 7]];
        if typ == *b"moov" {
            moov = Some((off, size));
        }
        if typ == *b"mdat" {
            mdat = Some((off, size));
        }
        if size == 0 {
            break;
        }
        off += size;
    }
    let (moov_off, moov_size) = moov.ok_or_else(|| anyhow!("MP4 中没有 moov 盒"))?;
    let loc = locate_insertion(&buf, moov_off, moov_off + moov_size)
        .ok_or_else(|| anyhow!("找不到视频采样条目（stsd→hvc1/hev1/avc1/av01），无法注入 HDR 盒"))?;
    let insert_len = 32 + 12;
    let mut insert = build_mdcv(&DEFAULT_MASTERING);
    insert.extend_from_slice(&build_clli(max_cll, max_fall));

    // 从最内层锚点整体插入，然后逐层补祖先盒 size
    let mut out = Vec::with_capacity(buf.len() + insert_len);
    out.extend_from_slice(&buf[..loc.1]);
    out.extend_from_slice(&insert);
    out.extend_from_slice(&buf[loc.1..]);
    for hdr_off in &loc.0 {
        add_box_size(&mut out, *hdr_off, insert_len as i64);
    }
    // moov 在 mdat 之前 → 插入使 mdat 后移 → 调整块偏移
    if moov_off < mdat.map(|(o, _)| o).unwrap_or(usize::MAX) {
        adjust_chunk_offsets(&mut out, moov_off, moov_off + moov_size + insert_len, insert_len as i64);
    }
    std::fs::write(path, out).with_context(|| format!("写入 MP4 失败: {}", path.display()))?;
    Ok(())
}

// ============================================================
//  视频转换主流程（← convertVideoFrames）
// ============================================================

/// 视频 → HDR10 mp4。
pub fn run_video(input: &Path, output: &Path, opts: &VideoOptions) -> Result<VideoOutcome> {
    let ffmpeg = find_tool(opts.ffmpeg.as_deref(), "ffmpeg")?;
    let ffprobe = find_tool(opts.ffprobe.as_deref(), "ffprobe")?;
    let info = probe_video(&ffprobe, input)?;
    if info.width == 0 || info.height == 0 {
        bail!("无法获取视频分辨率");
    }

    let peak_nits = opts.peak_nits;
    let white_nits = opts.white_nits;
    let peak = peak_nits / white_nits; // PAM 归一化峰值（白点倍率）
    let npl = peak_nits;
    let max_cll = peak_nits as u32;
    let crf = opts.crf;
    let fps = info.fps.max(0.1);
    let mode_label = match opts.mode {
        TransformMode::Transform => "单层 HDR 变换（ICC 增益式）",
        TransformMode::Gainmap => "增益图重建",
    };

    // 1) 解码为 PNG 帧序列（CPU 软解；可限宽）
    let out_base = pos_last_dot(output);
    let tmp_dir = PathBuf::from(format!("{out_base}_hdr_frames"));
    std::fs::create_dir_all(&tmp_dir).context("创建帧目录失败")?;

    let scale_vf = opts.max_width.map(|w| format!("scale='min({w},iw)':-2"));
    let mut extract = vec![
        "-y".to_string(),
        "-nostats".to_string(),
        "-i".to_string(),
        input.to_string_lossy().into_owned(),
    ];
    if let Some(vf) = &scale_vf {
        extract.extend(["-vf".to_string(), vf.clone()]);
    }
    extract.extend([
        "-f".to_string(),
        "image2".to_string(),
        "-start_number".to_string(),
        "0".to_string(),
        tmp_dir.join("frame_%06d.png").to_string_lossy().into_owned(),
    ]);
    let extract_args: Vec<&str> = extract.iter().map(|s| s.as_str()).collect();
    // NVDEC 硬解优先（← JS cuvidCodecs 集合），失败自动回退 CPU 软解
    const CUVID_CODECS: [&str; 10] = [
        "h264", "hevc", "av1", "mpeg2video", "mpeg1video", "mpeg4", "vc1", "vp8", "vp9", "mjpeg",
    ];
    println!("[video] 解码视频帧…");
    if CUVID_CODECS.contains(&info.codec.as_str()) {
        let mut hw = extract.clone();
        // -hwaccel cuda 必须位于 -i 之前（← JS extractArgs(true)）
        hw.insert(2, "-hwaccel".to_string());
        hw.insert(3, "cuda".to_string());
        let hw_args: Vec<&str> = hw.iter().map(|s| s.as_str()).collect();
        match run_capture(&ffmpeg, &hw_args) {
            Ok(_) => println!("[video] 解码使用 CUDA 硬件加速（{}）", info.codec),
            Err(e) => {
                println!("[video] CUDA 解码失败，回退 CPU 软解: {e:#}");
                run_capture(&ffmpeg, &extract_args).context("解码失败")?;
            }
        }
    } else {
        println!(
            "[video] 输入编码 {} 无 cuvid 解码器，使用 CPU 软解",
            if info.codec.is_empty() { "未知" } else { &info.codec }
        );
        run_capture(&ffmpeg, &extract_args).context("解码失败")?;
    }

    let mut frames: Vec<String> = std::fs::read_dir(&tmp_dir)
        .context("读取帧目录失败")?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.len() == 16 && n.starts_with("frame_") && n.ends_with(".png"))
        .collect();
    frames.sort();
    let total = frames.len();
    if total == 0 {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        bail!("视频解码失败：未生成任何帧");
    }

    // 2) 编码器选择（管道化后不可重放，先探测再启动）
    let x265_params = format!(
        "colorprim=bt2020:transfer=smpte2084:colormatrix=bt2020nc:{MASTER_DISPLAY}:max-cll={max_cll},400:repeat-headers=1:profile=main10"
    );
    let (encoder_name, enc_args) = pick_encoder(&ffmpeg, &opts.encoder, crf, &x265_params);

    // 3) 编码参数（与 JS encArgs 一致）
    let vf = format!(
        "zscale=in_range=full:pin=bt709:tin=linear:npl={npl}:p=bt2020:t=smpte2084:m=bt2020nc:r=limited,format=yuv420p10le"
    );
    let silent_out = tmp_dir.join("silent_hdr.mp4");
    let mut enc_args_full: Vec<String> = vec![
        "-y".into(),
        "-nostats".into(),
        "-f".into(),
        "pam_pipe".into(),
        "-framerate".into(),
        format!("{fps}"),
        "-i".into(),
        "pipe:0".into(),
        "-vf".into(),
        vf,
    ];
    enc_args_full.extend(enc_args);
    enc_args_full.extend([
        "-color_primaries".into(),
        "bt2020".into(),
        "-color_trc".into(),
        "smpte2084".into(),
        "-colorspace".into(),
        "bt2020nc".into(),
        "-color_range".into(),
        "tv".into(),
        "-an".into(),
        silent_out.to_string_lossy().into_owned(),
    ]);

    // 4) 逐帧重建（帧级并发）+ 按序喂入编码器 stdin
    println!("[video] 逐帧{mode_label} 0/{total}…（编码器 {encoder_name}）");
    let settings = Settings {
        peak_nits,
        white_nits,
        gamma: opts.gamma,
        ..Settings::default()
    };
    let mode = opts.mode;

    // 启动编码器（pam_pipe 明确格式，可先行 spawn；stdin 首次写入时已有数据）
    let mut child = Command::new(&ffmpeg)
        .args(&enc_args_full)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("启动 ffmpeg 编码器失败")?;
    let mut enc_stdin = child.stdin.take().context("编码器 stdin 不可用")?;
    let enc_stdout = child.stdout.take();
    let enc_stderr = child.stderr.take();
    let stderr_buf: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let stderr_buf_t = Arc::clone(&stderr_buf);
    if let Some(mut err) = enc_stderr {
        std::thread::spawn(move || {
            let mut s = String::new();
            if err.read_to_string(&mut s).is_ok() {
                stderr_buf_t.lock().unwrap().push(s);
            }
        });
    }
    // 消费 stdout（无 -progress，但须排空防阻塞）
    let stdout_done: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
    let stdout_done_t = Arc::clone(&stdout_done);
    if let Some(mut out) = enc_stdout {
        std::thread::spawn(move || {
            let mut sink = [0u8; 8192];
            while out.read(&mut sink).map(|n| n > 0).unwrap_or(false) {}
            stdout_done_t.store(true, Ordering::Relaxed);
        });
    }

    // 管道：worker 重建 → 主线程按序喂入
    let (tx, rx) = mpsc::sync_channel::<(usize, Vec<u8>)>(6); // 有界：防 GPU 泵快速产出导致 PAM 积压（4K 单帧 ~50MB）
    let next = Arc::new(AtomicUsize::new(0));
    let failed = Arc::new(AtomicBool::new(false));
    let workers_done = Arc::new(AtomicBool::new(false));
    // GPU 帧泵：pinned 双缓冲 + 多槽 stream（HDRCONV_GPU=1 + 可用时），共享锁保证槽位互斥
    let pump: Option<Arc<std::sync::Mutex<crate::gpu::FramePump>>> = if crate::gpu::gpu_enabled()
        && crate::gpu::gpu_available()
    {
        crate::gpu::FramePump::try_new(tx.clone()).map(|p| Arc::new(std::sync::Mutex::new(p)))
    } else {
        None
    };
    let _workers = {
        let n_workers = opts.jobs.unwrap_or_else(|| {
            std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(1, 8)
        });
        (1..=n_workers).map(|_| {
            let tx = tx.clone();
            let next = Arc::clone(&next);
            let failed = Arc::clone(&failed);
            let workers_done = Arc::clone(&workers_done);
            let pump = pump.clone();
            let tmp_dir = tmp_dir.clone();
            let settings = settings.clone();
            let frames = frames.clone();
            std::thread::spawn(move || {
                while !failed.load(Ordering::Relaxed) {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= frames.len() {
                        break;
                    }
                    let path = tmp_dir.join(&frames[i]);
                    let img = match crate::convert::read_image_rgba(&path) {
                        Ok(img) => img,
                        Err(e) => {
                            failed.store(true, Ordering::Relaxed);
                            let _ = tx.send((usize::MAX, format!("帧 {i} 读取失败: {e:#}").into_bytes()));
                            break;
                        }
                    };
                    // 泵优先（异步提交，结果经 channel 回传）；否则走同步重建
                    if let Some(pump) = &pump {
                        let params: Box<[f64]> = match mode {
                            TransformMode::Gainmap => {
                                Box::new([settings.gain_ev(), settings.gamma, peak])
                            }
                            TransformMode::Transform => Box::new([
                                peak,
                                settings.gamma,
                                settings.rgb.red,
                                settings.rgb.green,
                                settings.rgb.blue,
                                peak,
                            ]),
                        };
                        let mode_num = match mode {
                            TransformMode::Gainmap => crate::gpu::FrameMode::Gainmap16,
                            TransformMode::Transform => crate::gpu::FrameMode::Transform16,
                        };
                        let done = {
                            // 锁内只提交+逐出；发送在锁外（有界通道可能阻塞）
                            let mut p = pump.lock().unwrap();
                            p.submit(i, &img.pixels, img.width, img.height, mode_num, &params)
                        };
                        match done {
                            Ok(done) => {
                                for (f, pam) in done {
                                    if tx.send((f, pam)).is_err() {
                                        failed.store(true, Ordering::Relaxed);
                                        break;
                                    }
                                }
                            }
                            Err(e) => {
                                failed.store(true, Ordering::Relaxed);
                                let _ = tx.send((usize::MAX, format!("帧 {i} GPU 泵提交失败: {e:#}").into_bytes()));
                                break;
                            }
                        }
                        continue;
                    }
                    // CPU / 同步 GPU 路径
                    let result: Result<Vec<u8>> = (|| {
                        let pam = match mode {
                            TransformMode::Gainmap => {
                                if let Some(px) = crate::gpu::try_gpu_reconstruct_gainmap16_pixels(
                                    &img.pixels, img.width, img.height, settings.gain_ev(), settings.gamma, peak,
                                ) {
                                    ultra_hdr::pam_with_pixels(img.width, img.height, &px)
                                } else {
                                    ultra_hdr::reconstruct_linear_hdr_frame(
                                        &img.pixels, img.width, img.height, &settings, peak, settings.gain_ev(),
                                    )?
                                }
                            }
                            TransformMode::Transform => {
                                if let Some(px) = crate::gpu::try_gpu_reconstruct_transform16_pixels(
                                    &img.pixels,
                                    img.width,
                                    img.height,
                                    peak,
                                    settings.gamma,
                                    settings.rgb.red,
                                    settings.rgb.green,
                                    settings.rgb.blue,
                                    peak,
                                ) {
                                    ultra_hdr::pam_with_pixels(img.width, img.height, &px)
                                } else {
                                    ultra_hdr::reconstruct_linear_hdr_transform(
                                        &img.pixels, img.width, img.height, &settings, peak,
                                    )?
                                }
                            }
                        };
                        Ok(pam)
                    })();
                    match result {
                        Ok(pam) => {
                            if tx.send((i, pam)).is_err() {
                                failed.store(true, Ordering::Relaxed);
                                break;
                            }
                        }
                        Err(e) => {
                            failed.store(true, Ordering::Relaxed);
                            let _ = tx.send((usize::MAX, format!("帧 {i} 重建失败: {e:#}").into_bytes()));
                            break;
                        }
                    }
                }
                workers_done.store(true, Ordering::SeqCst);
            })
        }).collect::<Vec<_>>()
    };

    // 主线程：乱序缓冲按序号顺序喂入（轮询：channel + 泵 flush 收尾）
    let mut buf: std::collections::BTreeMap<usize, Vec<u8>> = std::collections::BTreeMap::new();
    let mut next_idx = 0usize;
    let mut processed = 0usize;
    let mut feed_err: Option<anyhow::Error> = None;
    loop {
        // 收尾：所有 worker 结束后 flush 泵内剩余帧 + 释放 channel 发送端
        if workers_done.load(Ordering::SeqCst) {
            if let Some(pump) = &pump {
                let done = {
                    let mut p = pump.lock().unwrap();
                    let d = p.flush();
                    p.close_tx();
                    d
                };
                for (i, pam) in done {
                    if i == usize::MAX {
                        feed_err = Some(anyhow!(String::from_utf8_lossy(&pam).into_owned()));
                        break;
                    }
                    if let Err(e) = feed_reorder(
                        &mut buf,
                        &mut next_idx,
                        &mut processed,
                        total,
                        &mut enc_stdin,
                        mode_label,
                        i,
                        pam,
                        &failed,
                    ) {
                        feed_err = Some(e);
                        break;
                    }
                }
            }
        }
        loop {
            match rx.try_recv() {
                Ok((i, pam)) => {
                    if i == usize::MAX {
                        feed_err = Some(anyhow!(String::from_utf8_lossy(&pam).into_owned()));
                        break;
                    }
                    if let Err(e) = feed_reorder(
                        &mut buf,
                        &mut next_idx,
                        &mut processed,
                        total,
                        &mut enc_stdin,
                        mode_label,
                        i,
                        pam,
                        &failed,
                    ) {
                        feed_err = Some(e);
                        break;
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
            }
        }
        if feed_err.is_some() {
            break;
        }
        if processed == total {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    // 收尾：关闭 stdin，等编码器结束
    drop(enc_stdin);
    failed.store(true, Ordering::Relaxed); // 通知 worker 停止（防泄漏）
    let status = child.wait().context("等待编码器失败")?;
    if !status.success() {
        let err_tail = stderr_buf.lock().unwrap().join("\n");
        let _ = std::fs::remove_dir_all(&tmp_dir);
        bail!(
            "ffmpeg 编码退出码 {:?}: {}",
            status.code(),
            err_tail.chars().rev().take(600).collect::<String>().chars().rev().collect::<String>()
        );
    }
    if let Some(e) = feed_err {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(e.context("逐帧重建中断"));
    }
    println!("[video] 编码完成，帧数 {total}");

    // 4.5) nvenc 归一化编码高度补边（仅 hevc_nvenc 会按 32 对齐补边）
    let mut mux_source = silent_out.clone();
    if encoder_name == "nvenc" {
        let silent_info = probe_video(&ffprobe, &silent_out).unwrap_or_else(|_| {
            // coded_height 缺省 == height
            ProbeInfo { width: info.width, height: info.height, coded_height: info.height, ..Default::default() }
        });
        if silent_info.coded_height != silent_info.height {
            println!("[video] 检测到 NVENC 编码高度补边 {}→{}，执行归一化重编码…", silent_info.height, silent_info.coded_height);
            let norm_out = tmp_dir.join("silent_hdr_norm.mp4");
            let x265 = format!(
                "colorprim=bt2020:transfer=smpte2084:colormatrix=bt2020nc:{MASTER_DISPLAY}:max-cll={max_cll},400:repeat-headers=1:profile=main10"
            );
            let args = [
                "-y", "-nostats", "-i", silent_out.to_str().unwrap_or(""),
                "-c:v", "libx265", "-preset", "medium", "-crf", "18", "-tag:v", "hvc1",
                "-x265-params", &x265,
                "-color_primaries", "bt2020", "-color_trc", "smpte2084", "-colorspace", "bt2020nc",
                "-color_range", "tv", "-an", norm_out.to_str().unwrap_or(""),
            ];
            match run_capture(&ffmpeg, &args) {
                Ok(_) => mux_source = norm_out,
                Err(e) => println!("[video] 归一化失败，沿用原产物: {e:#}"),
            }
        }
    }

    // 5) 合并原音频（尽力而为，失败则保留无声版）
    let _mux_err = (|| -> Result<()> {
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let args = [
            "-y", "-nostats", "-i", mux_source.to_str().unwrap_or(""),
            "-i", input.to_str().unwrap_or(""),
            "-map", "0:v:0", "-map", "1:a:0?",
            "-c:v", "copy", "-c:a", "aac", "-b:a", "160k", "-shortest",
            output.to_str().unwrap_or(""),
        ];
        match run_capture(&ffmpeg, &args) {
            Ok(_) => Ok(()),
            Err(_) => {
                // 失败 → 直接拷贝无声版
                std::fs::copy(&mux_source, output).context("拷贝无声版失败")?;
                Ok(())
            }
        }
    })();

    // 6) 注入 mdcv / clli 容器盒（Chromium demuxer 依赖）
    inject_hdr_boxes(output, max_cll as u16, 400).context("注入 mdcv/clli 失败")?;

    // 7) Eclipsa（ST 2094-50 动态元数据；仅 HEVC 输出支持）
    if opts.eclipsa {
        if !matches!(encoder_name.as_str(), "x265" | "nvenc") {
            println!(
                "[video] Eclipsa 仅支持 HEVC（x265/nvenc），当前为 {}，保持 HDR10",
                encoder_name
            );
        } else {
            println!("[video] 附加 ST 2094-50 动态元数据（Eclipsa）…");
            let tmp_hdr = PathBuf::from(format!("{}.hdr10_tmp.mp4", pos_last_dot(output)));
            let eclipsa_opts = crate::eclipsa::EclipsaOptions {
                ref_white_nits: white_nits,
                max_cll: max_cll as u16,
                max_fall: 400,
                scheme: if opts.eclipsa_scheme == "uniform" {
                    crate::eclipsa::WindowScheme::Uniform
                } else {
                    crate::eclipsa::WindowScheme::Scene
                },
                uniform_windows: opts.eclipsa_windows.max(1),
                scene_threshold: 0.4,
                min_window_sec: 0.5,
                ffmpeg: ffmpeg.clone(),
                ffprobe: ffprobe.clone(),
            };
            match (std::fs::rename(output, &tmp_hdr), crate::eclipsa::attach_eclipsa(&tmp_hdr, output, &eclipsa_opts)) {
                (Ok(_), Ok(outcome)) => {
                    println!(
                        "[video] Eclipsa 完成：{} 窗 / {} 条 ST 2094-50 SEI",
                        outcome.windows.len(),
                        outcome.total_sei
                    );
                }
                _ => {
                    // 回退 HDR10：temp 换回 output
                    if tmp_hdr.exists() && !output.exists() {
                        let _ = std::fs::rename(&tmp_hdr, output);
                    }
                    println!("[video] Eclipsa 附加失败，已回退 HDR10");
                }
            }
            let _ = std::fs::remove_file(tmp_hdr);
        }
    }

    // 清理
    let _ = std::fs::remove_dir_all(&tmp_dir);
    Ok(VideoOutcome {
        encoder_used: encoder_name,
        width: info.width,
        height: info.height,
        frames: total,
        fps,
    })
}

/// 输出基础名：去掉最后一个扩展名。
pub(crate) fn pos_last_dot(p: &Path) -> String {
    let s = p.to_string_lossy();
    match s.rfind('.') {
        Some(i) => s[..i].to_string(),
        None => s.to_string(),
    }
}

/// 乱序缓冲 → 按序号喂入编码器 stdin（携记录进度）；写失败返回 Err（调用方 abort）。
#[allow(clippy::too_many_arguments)]
fn feed_reorder(
    buf: &mut std::collections::BTreeMap<usize, Vec<u8>>,
    next_idx: &mut usize,
    processed: &mut usize,
    total: usize,
    enc_stdin: &mut std::process::ChildStdin,
    mode_label: &str,
    i: usize,
    pam: Vec<u8>,
    failed: &AtomicBool,
) -> Result<()> {
    buf.insert(i, pam);
    loop {
        let cur = *next_idx;
        let Some(pam) = buf.remove(&cur) else { break };
        if let Err(e) = enc_stdin.write_all(&pam) {
            failed.store(true, Ordering::Relaxed);
            return Err(anyhow!("写入编码器 stdin 失败: {e:#}"));
        }
        *next_idx += 1;
        *processed += 1;
        if *processed % 25 == 0 || *processed == total {
            println!("[video] 逐帧{mode_label} {processed}/{total}");
        }
    }
    Ok(())
}