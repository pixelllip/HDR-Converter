//! ← st2094_50_inject.js + hevc_inject.js：给 HDR10（HEVC）MP4 附加 ST 2094-50 动态元数据。
//!
//! 流程（与 JS 一致）：
//!   signalstats 逐帧 YMAX → PQ EOTF → 逐窗 MaxCLL(尼特) → Hbaseline=log2(MaxCLL/参考白)
//!   → 参考白配方载荷 → mp4→AnnexB(补 AUD) → 按 AUD 注入 Prefix_SEI
//!   → remux 回 mp4 → 补回 mdcv/clli 盒（复用 video::inject_hdr_boxes）。

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};

use crate::st2094_50;
use crate::video;

/// 窗口划分方案。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowScheme {
    Scene,
    Uniform,
}

/// Eclipsa 附加参数（← attachSt2094_50 opts）。
#[derive(Debug, Clone)]
pub struct EclipsaOptions {
    pub ref_white_nits: f64,
    pub max_cll: u16,
    pub max_fall: u16,
    pub scheme: WindowScheme,
    pub uniform_windows: usize,
    pub scene_threshold: f64,
    pub min_window_sec: f64,
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
}

impl Default for EclipsaOptions {
    fn default() -> Self {
        Self {
            ref_white_nits: 203.0,
            max_cll: 574,
            max_fall: 400,
            scheme: WindowScheme::Scene,
            uniform_windows: 3,
            scene_threshold: 0.4,
            min_window_sec: 0.5,
            ffmpeg: PathBuf::from("backend/ffmpeg/ffmpeg.exe"),
            ffprobe: PathBuf::from("backend/ffmpeg/ffprobe.exe"),
        }
    }
}

/// 结果（← attachSt2094_50 返回）。
#[derive(Debug)]
pub struct EclipsaOutcome {
    pub windows: Vec<EclipsaWindow>,
    pub total_sei: usize,
}

#[derive(Debug)]
pub struct EclipsaWindow {
    pub start_frame: usize,
    pub end_frame: usize,
    pub max_cll_nits: u32,
    pub h_baseline: f64,
    pub raw: u32,
}

fn sh(bin: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new(bin)
        .args(args)
        .output()
        .with_context(|| format!("运行失败: {} {}", bin.display(), args.join(" ")))?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let combined = format!("{stdout}\n{stderr}");
    if !out.status.success() {
        bail!(
            "{} 退出码 {}: {}",
            bin.display(),
            out.status.code().unwrap_or(-1),
            combined.chars().rev().take(600).collect::<String>().chars().rev().collect::<String>()
        );
    }
    Ok(combined)
}

/// 逐帧 YMAX（10-bit limited PQ 码值 0..1023；← perFrameYMax）。
fn per_frame_y_max(ffmpeg: &Path, mp4: &Path) -> Result<Vec<f64>> {
    let txt = sh(
        ffmpeg,
        &[
            "-hide_banner", "-i", mp4.to_str().unwrap_or(""),
            "-vf", "signalstats,metadata=print:key=lavfi.signalstats.YMAX",
            "-an", "-f", "null", "-",
        ],
    )?;
    let mut frames = Vec::new();
    for cap in txt.split("lavfi.signalstats.YMAX=").skip(1) {
        let v: String = cap.chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(n) = v.parse::<f64>() {
            frames.push(n);
        }
    }
    Ok(frames)
}

/// 镜头切检测：返回切割帧下标（← sceneCuts）。
fn scene_cuts(ffmpeg: &Path, mp4: &Path, fps: f64, threshold: f64) -> Result<Vec<usize>> {
    let txt = sh(
        ffmpeg,
        &[
            "-hide_banner", "-i", mp4.to_str().unwrap_or(""),
            "-vf", &format!("select='gt(scene,{threshold})',showinfo"),
            "-an", "-f", "null", "-",
        ],
    )?;
    let mut times = Vec::new();
    for part in txt.split("pts_time:") {
        let v: String = part
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        if let Ok(t) = v.parse::<f64>() {
            times.push(t);
        }
    }
    Ok(times
        .iter()
        .map(|t| (t * fps).round().max(0.0) as usize)
        .collect())
}

/// 窗口划分（← buildWindows）。
fn build_windows(
    frame_count: usize,
    fps: f64,
    cuts: &[usize],
    scheme: WindowScheme,
    uniform_windows: usize,
    min_window_sec: f64,
) -> Vec<(usize, usize)> {
    let mut bounds: Vec<usize> = Vec::new();
    if scheme == WindowScheme::Scene && !cuts.is_empty() {
        let min_frames = (min_window_sec * fps).round().max(1.0) as usize;
        for &c in cuts {
            if c >= min_frames
                && c <= frame_count.saturating_sub(min_frames)
                && (bounds.is_empty() || c - bounds[bounds.len() - 1] >= min_frames)
            {
                bounds.push(c);
            }
        }
    }
    if bounds.iter().filter(|&&b| b > 0 && b < frame_count).count() < 1 {
        let n = uniform_windows.max(1);
        for w in 1..n {
            bounds.push((frame_count * w) / n);
        }
    }
    let mut wins = Vec::new();
    let mut start = 0usize;
    for b in bounds.iter().chain(std::iter::once(&frame_count)) {
        let end = (*b).min(frame_count);
        if end > start {
            wins.push((start, end));
            start = end;
        }
    }
    if wins.is_empty() {
        wins.push((0, frame_count));
    }
    wins
}

// ============================================================
//  Annex B NAL 分割与按 AUD 注入（← hevc_inject.js）
// ============================================================

fn split_nal_units(buf: &[u8]) -> Vec<(usize, usize)> {
    let mut starts = Vec::new();
    let mut i = 0usize;
    while i + 3 < buf.len() {
        if buf[i] == 0 && buf[i + 1] == 0 {
            if buf[i + 2] == 1 {
                starts.push(i);
                i += 3;
                continue;
            }
            if i + 3 < buf.len() && buf[i + 2] == 0 && buf[i + 3] == 1 {
                starts.push(i);
                i += 4;
                continue;
            }
        }
        i += 1;
    }
    let mut nals = Vec::with_capacity(starts.len());
    for k in 0..starts.len() {
        let end = if k + 1 < starts.len() { starts[k + 1] } else { buf.len() };
        nals.push((starts[k], end));
    }
    nals
}

fn nal_type(buf: &[u8], start: usize) -> u8 {
    let mut p = start;
    if buf[p] == 0 && buf[p + 1] == 0 {
        p = if buf[p + 2] == 1 { p + 3 } else { p + 4 };
    }
    (buf[p] >> 1) & 0x3F
}

/// 按 AUD（type=35）注入 Prefix_SEI（← injectSeiPerAu，position=after-aud）。
fn inject_sei_per_au(src: &[u8], payload_for_au: impl Fn(usize) -> Vec<u8>) -> Result<Vec<u8>> {
    let nals = split_nal_units(src);
    let aud_idx: Vec<usize> = (0..nals.len()).filter(|&i| nal_type(src, nals[i].0) == 35).collect();
    if aud_idx.is_empty() {
        bail!("未找到 AUD（建议 x265 加 -x265-params aud=1 重新生成）");
    }
    let mut out = Vec::with_capacity(src.len() + aud_idx.len() * 24);
    let mut au = 0usize;
    for &(start, end) in nals.iter() {
        let is_aud = nal_type(src, start) == 35;
        out.extend_from_slice(&src[start..end]);
        if is_aud {
            // 4 字节开始码 + Prefix_SEI（← JS：Buffer.concat([0,0,0,1], build(payload))）
            let payload = payload_for_au(au);
            let sei = st2094_50::build_prefix_sei_nal(&payload);
            out.extend_from_slice(&[0, 0, 0, 1]);
            out.extend_from_slice(&sei);
            au += 1;
        }
    }
    Ok(out)
}

// ============================================================
//  总入口（← attachSt2094_50）
// ============================================================

/// 给 HDR10（HEVC）MP4 附加 ST 2094-50 动态元数据，输出到 outputPath。
pub fn attach_eclipsa(input: &Path, output: &Path, opts: &EclipsaOptions) -> Result<EclipsaOutcome> {
    let work = std::env::temp_dir().join(format!("hdr_eclipsa_{}", std::process::id()));
    std::fs::create_dir_all(&work).context("创建临时目录失败")?;

    let result = (|| -> Result<EclipsaOutcome> {
        // 1) 逐帧 YMAX + fps（← perFrameYMax + ffprobe）
        let ymax = per_frame_y_max(&opts.ffmpeg, input)?;
        let frame_count = ymax.len();
        if frame_count == 0 {
            bail!("signalstats 未读到帧数");
        }
        let mut fps = 30.0f64;
        let fps_out = std::process::Command::new(&opts.ffprobe)
            .args([
                "-v", "error", "-select_streams", "v:0",
                "-show_entries", "stream=avg_frame_rate",
                "-of", "default=noprint_wrappers=1:nokey=1",
                input.to_str().unwrap_or(""),
            ])
            .output()?;
        if fps_out.status.success() {
            let s = String::from_utf8_lossy(&fps_out.stdout);
            let s = s.trim();
            if let Some(slash) = s.find('/') {
                if let (Ok(a), Ok(b)) = (s[..slash].parse::<f64>(), s[slash + 1..].parse::<f64>()) {
                    if b > 0.0 {
                        fps = a / b;
                    }
                }
            } else if let Ok(v) = s.parse::<f64>() {
                fps = v;
            }
        }
        if !(fps > 0.0) {
            fps = 30.0;
        }

        // 2) 镜头切（scene 方案）→ 窗口
        let cuts = if opts.scheme == WindowScheme::Scene {
            scene_cuts(&opts.ffmpeg, input, fps, opts.scene_threshold)?
        } else {
            vec![]
        };
        let windows = build_windows(
            frame_count,
            fps,
            &cuts,
            opts.scheme,
            opts.uniform_windows,
            opts.min_window_sec,
        );

        // 3) 每窗 MaxCLL → Hbaseline → 参考白配方载荷
        let mut payloads: Vec<(usize, usize, u32, f64, u32, Vec<u8>)> = Vec::with_capacity(windows.len());
        for (start, end) in &windows {
            let mut mx = 0.0f64;
            for i in *start..*end {
                let v = st2094_50::pq_eotf(ymax[i] / 1023.0);
                if v > mx {
                    mx = v;
                }
            }
            let nits = mx.round() as u32;
            let hb = if nits > 0 {
                (nits as f64 / opts.ref_white_nits).log2()
            } else {
                0.0
            };
            let raw = ((hb.min(6.0).max(0.0) * 10000.0).round() as u32).max(0);
            let payload = st2094_50::t35_payload(&st2094_50::reference_white_app_info(raw as u16));
            payloads.push((*start, *end, nits, hb, raw, payload));
        }

        // 4) mp4 → AnnexB（补 AUD + 提取裸流）
        let es_in = work.join("in.h265");
        sh(
            &opts.ffmpeg,
            &[
                "-hide_banner", "-y", "-i", input.to_str().unwrap_or(""),
                "-c", "copy", "-bsf:v", "hevc_mp4toannexb,hevc_metadata=aud=insert",
                "-f", "hevc", es_in.to_str().unwrap_or(""),
            ],
        )
        .context("AnnexB 转换失败")?;
        let es_buf = std::fs::read(&es_in).context("读取裸流失败")?;

        // 5) 按 AUD 注入（AU 序号 == 帧序号，源自 same frame counting）
        let payload_for_au = |au: usize| -> Vec<u8> {
            payloads
                .iter()
                .find(|p| au >= p.0 && au < p.1)
                .map(|p| p.5.clone())
                .unwrap_or_else(|| payloads.last().map(|p| p.5.clone()).unwrap_or_default())
        };
        let injected = inject_sei_per_au(&es_buf, payload_for_au)?;
        let es_out = work.join("injected.h265");
        std::fs::write(&es_out, &injected)?;

        // 6) remux 回 mp4（hvc1 + avoid_negative_ts + faststart）
        sh(
            &opts.ffmpeg,
            &[
                "-hide_banner", "-y", "-i", es_out.to_str().unwrap_or(""),
                "-c", "copy", "-tag:v", "hvc1",
                "-avoid_negative_ts", "make_zero",
                "-movflags", "+faststart",
                output.to_str().unwrap_or(""),
            ],
        )
        .context("remux 失败")?;

        // 7) 补回容器静态盒（mdcv / clli）
        video::inject_hdr_boxes(output, opts.max_cll, opts.max_fall)?;

        let total_sei = split_nal_units(&es_buf)
            .iter()
            .filter(|&&(s, _)| nal_type(&es_buf, s) == 35)
            .count();
        Ok(EclipsaOutcome {
            windows: payloads
                .into_iter()
                .map(|(s, e, nits, hb, raw, _)| EclipsaWindow {
                    start_frame: s,
                    end_frame: e - 1,
                    max_cll_nits: nits,
                    h_baseline: hb,
                    raw,
                })
                .collect(),
            total_sei,
        })
    })();

    let _ = std::fs::remove_dir_all(&work);
    result.map_err(|e| anyhow!("Eclipsa 附加失败: {e:#}"))
}