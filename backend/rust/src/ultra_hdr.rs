//! ← UltraHdrEncoder.kt：Ultra HDR 编码器 + 视频逐帧重建（Kotlin 1032 行的 1:1 移植）。
//!
//! 逐位对齐的部分：增益图数学（computeGainMap）、sRGB→Display-P3 主色转换、
//! XMP / MPF / ICC 段组装（buildXmpPrimary/Secondary、buildMpfPayload、reorderPrimary、buildSrgbIcc）。
//!
//! 与 Kotlin 的有意差异：
//! - **JPEG 编码器不同**（image crate vs ImageIO）：结构一致、可解码、增益图数学与
//!   XMP 数值完全一致，但 JPEG 字节流不会逐位相同（有损编码差异）——因此 ultra-hdr
//!   不做逐字节对照，改为「XMP 数值对照」（增益图统计量在 JPEG 编码前算出，本应一致）。
//! - GPU 路径未接入（feature "gpu" 关闭时回退 CPU；CPU 实现与 Kotlin CPU 逐位一致）。
//! - 线程模型：Kotlin 用多线程并行，本实现顺序执行——像素计算无跨点依赖，结果一致。

use anyhow::{Context, Result};
use base64::Engine as _;
use rayon::prelude::*;
use std::sync::OnceLock;

use crate::colorspace::InputColorSpace;
use crate::models::Settings;

// ============================================================
//  ICC 配置文件（取自真实 Google Ultra HDR 文件，与 Kotlin 常量一致）
// ============================================================

/// 主图像 ICC：Display-P3 基色 + sRGB 传递函数（Google Inc. 2016）
const UHDR_PRIMARY_ICC_B64: &str = "AAACCAAAAAAEMAAAbW50clJHQiBYWVogB+AAAQABAAAAAAAAYWNzcAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAEAAPbWAAEAAAAA0y0AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAJZGVzYwAAAPAAAABkclhZWgAAAVQAAAAUZ1hZWgAAAWgAAAAUYlhZWgAAAXwAAAAUd3RwdAAAAZAAAAAUclRSQwAAAaQAAAAoZ1RSQwAAAaQAAAAoYlRSQwAAAaQAAAAoY3BydAAAAcwAAAA8bWx1YwAAAAAAAAABAAAADGVuVVMAAABGAAAAHABEAGkAcwBwAGwAYQB5ACAAUAAzACAARwBhAG0AdQB0ACAAdwBpAHQAaAAgAHMAUgBHAEIAIABUAHIAYQBuAHMAZgBlAHIAAFhZWiAAAAAAAACD3wAAPb////+7WFlaIAAAAAAAAEq/AACxNwAACrlYWVogAAAAAAAAKDgAABELAADIuVhZWiAAAAAAAAD21gABAAAAANMtcGFyYQAAAAAABAAAAAJmZgAA8qcAAA1ZAAAT0AAAClsAAAAAAAAAAG1sdWMAAAAAAAAAAQAAAAxlblVTAAAAIAAAABwARwBvAG8AZwBsAGUAIABJAG4AYwAuACAAMgAwADEANg==";

/// 增益图 ICC：sRGB（Google Inc. 2016）
const UHDR_GAINMAP_ICC_B64: &str = "AAAByAAAAAAEMAAAbW50clJHQiBYWVogB+AAAQABAAAAAAAAYWNzcAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAEAAPbWAAEAAAAA0y0AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAJZGVzYwAAAPAAAAAkclhZWgAAARQAAAAUZ1hZWgAAASgAAAAUYlhZWgAAATwAAAAUd3RwdAAAAVAAAAAUclRSQwAAAWQAAAAoZ1RSQwAAAWQAAAAoYlRSQwAAAWQAAAAoY3BydAAAAYwAAAA8bWx1YwAAAAAAAAABAAAADGVuVVMAAAAIAAAAHABzAFIARwBCWFlaIAAAAAAAAG+iAAA49QAAA5BYWVogAAAAAAAAYpkAALeFAAAY2lhZWiAAAAAAAAAkoAAAD4QAALbPWFlaIAAAAAAAAPbWAAEAAAAA0y1wYXJhAAAAAAAEAAAAAmZmAADypwAADVkAABPQAAAKWwAAAAAAAAAAbWx1YwAAAAAAAAABAAAADGVuVVMAAAAgAAAAHABHAG8AbwBnAGwAZQAgAEkAbgBjAC4AIAAyADAAMQA2";

fn uhdr_primary_icc() -> &'static [u8] {
    static V: OnceLock<Vec<u8>> = OnceLock::new();
    V.get_or_init(|| {
        base64::engine::general_purpose::STANDARD
            .decode(UHDR_PRIMARY_ICC_B64)
            .expect("UHDR_PRIMARY_ICC base64 解码失败")
    })
}

fn uhdr_gainmap_icc() -> &'static [u8] {
    static V: OnceLock<Vec<u8>> = OnceLock::new();
    V.get_or_init(|| {
        base64::engine::general_purpose::STANDARD
            .decode(UHDR_GAINMAP_ICC_B64)
            .expect("UHDR_GAINMAP_ICC base64 解码失败")
    })
}

// ============================================================
//  基础工具（← UltraHdrEncoder.kt:45-137）
// ============================================================

fn srgb_to_linear(v: f64) -> f64 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(v: f64) -> f64 {
    if v <= 0.0031308 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

fn lum(r: f64, g: f64, b: f64) -> f64 {
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

fn clamp(v: f64, min: f64, max: f64) -> f64 {
    v.max(min).min(max)
}

fn read_u16(b: &[u8], off: usize) -> u16 {
    ((b[off] as u16) << 8) | (b[off + 1] as u16)
}

fn concat(parts: &[&[u8]]) -> Vec<u8> {
    let total: usize = parts.iter().map(|p| p.len()).sum();
    let mut out = Vec::with_capacity(total);
    for p in parts {
        out.extend_from_slice(p);
    }
    out
}

fn s15(v: f64) -> i32 {
    (v * 65536.0).round() as i32
}

/// ≈ Java `Double.toString`（对整数值补 ".0"，≥8 位小数按 round(×1e8)/1e8 截断；← Kotlin fmt）。
fn fmt_xmp(v: f64) -> String {
    if !v.is_finite() {
        return "0".to_string();
    }
    let r = (v * 1e8).round() / 1e8;
    let mut s = format!("{r:.8}");
    while s.ends_with('0') {
        s.pop();
    }
    if s.ends_with('.') {
        s.pop();
    }
    if r.fract() == 0.0 && !s.contains('.') {
        s.push_str(".0");
    }
    s
}

/// 增益图元数据（hdrgm XMP 原始值，min/max 为 content boost 线性倍数，非 log2；← Kotlin 行 94）。
#[derive(Debug, Clone, Copy)]
pub struct GainMapMetadata {
    pub min_content_boost: f64,
    pub max_content_boost: f64,
    pub gamma: f64,
    pub offset_sdr: f64,
    pub offset_hdr: f64,
    pub hdr_capacity_min: f64,
    pub hdr_capacity_max: f64,
}

// ============================================================
//  sRGB → Display-P3（← srgbRgbaToDisplayP3Rgba，行 56）
// ============================================================

/// 将 sRGB 编码 RGBA 转 Display-P3（sRGB 传递函数）编码 RGBA（CPU 并行路径，逐位对齐 Kotlin）。
fn srgb_rgba_to_display_p3_rgba(rgba: &[u8], width: usize, height: usize) -> Vec<u8> {
    let n = width * height;
    let mut out = vec![0u8; n * 4];
    out.par_chunks_exact_mut(4)
        .enumerate()
        .for_each(|(i, px)| {
            let base = i * 4;
            let r = srgb_to_linear(rgba[base] as f64 / 255.0);
            let g = srgb_to_linear(rgba[base + 1] as f64 / 255.0);
            let b = srgb_to_linear(rgba[base + 2] as f64 / 255.0);
            // sRGB 线性 -> Display-P3 线性（D65 到 D65）
            let pr = 0.82246 * r + 0.17749 * g + 0.00005 * b;
            let pg = 0.03311 * r + 0.96687 * g + 0.00002 * b;
            let pb = 0.01709 * r + 0.07239 * g + 0.91053 * b;
            px[0] = (linear_to_srgb(clamp(pr, 0.0, 1.0)) * 255.0)
                .round()
                .clamp(0.0, 255.0) as u8;
            px[1] = (linear_to_srgb(clamp(pg, 0.0, 1.0)) * 255.0)
                .round()
                .clamp(0.0, 255.0) as u8;
            px[2] = (linear_to_srgb(clamp(pb, 0.0, 1.0)) * 255.0)
                .round()
                .clamp(0.0, 255.0) as u8;
            px[3] = 255;
        });
    out
}

// ============================================================
//  增益图生成（← computeGainMap，行 369）
// ============================================================

/// 生成增益图（真正的高光扩展）。主图像 = SDR 底图；增益图 8-bit 全长分辨率。
///
/// maxBoost = min(2^hdrIntensity, 峰值/白点)，hdrIntensity 取 `settings.ev()`
/// （CLI 由峰值/白点推导，与 Electron UI 滑块语义一致）。
/// 返回 (增益图 8-bit, 元数据)。
pub fn compute_gain_map(
    primary_rgba: &[u8],
    width: usize,
    height: usize,
    settings: &Settings,
) -> (Vec<u8>, GainMapMetadata) {
    let hdr_intensity = settings.gain_ev(); // 显式 EV，否则峰值联动 log2(峰值/白点)
    let gamma = settings.gamma;
    let user_max_boost = 2.0f64.powf(hdr_intensity).clamp(1.0, 64.0);
    let white_nits = settings.white_nits;
    let peak_cap = (settings.peak_nits / white_nits).max(1.0);
    let max_boost = user_max_boost.min(peak_cap);
    let highlight_start = 0.5;
    let offset = 1.0 / 64.0;

    let n = width * height;
    // 并行：像素计算无跨点依赖，分段独立求 min/max；结果与顺序执行逐位一致
    let gain: Vec<f64> = (0..n)
        .into_par_iter()
        .map(|i| {
            let base = i * 4;
            let r = srgb_to_linear(primary_rgba[base] as f64 / 255.0);
            let g = srgb_to_linear(primary_rgba[base + 1] as f64 / 255.0);
            let b = srgb_to_linear(primary_rgba[base + 2] as f64 / 255.0);
            let y = lum(r, g, b);
            // 高光掩膜：亮度 > 50% 从 gain=1 渐变到 maxBoost（50% 以下 gain=1，保中间调）
            let mask = clamp((y - highlight_start) / (1.0 - highlight_start), 0.0, 1.0).powf(gamma);
            let gain_per_pix = 1.0 + (max_boost - 1.0) * mask;
            let yhdr = y * gain_per_pix;
            (yhdr + offset) / (y + offset)
        })
        .collect();
    let gmin = gain.iter().copied().fold(f64::INFINITY, f64::min);
    let gmax = gain.iter().copied().fold(f64::NEG_INFINITY, f64::max);

    let min_boost = gmin.min(1.0).clamp(0.25, 1.0); // Kotlin: clamp(Math.min(1.0, gmin), 0.25, 1.0)
    let max_boost_actual = gmax.max(1.0);
    let map_min = min_boost.log2();
    let map_max = max_boost_actual.log2();
    let range = (map_max - map_min).max(1e-6);

    let gm8: Vec<u8> = (0..n)
        .into_par_iter()
        .map(|i| {
            let log_rec = (gain[i].log2() - map_min) / range;
            let rec = log_rec.clamp(0.0, 1.0);
            (rec * 255.0).round().clamp(0.0, 255.0) as u8
        })
        .collect();

    let meta = GainMapMetadata {
        min_content_boost: min_boost,
        max_content_boost: max_boost_actual,
        gamma: 1.0,
        offset_sdr: offset,
        offset_hdr: offset,
        hdr_capacity_min: 1.0,
        hdr_capacity_max: max_boost_actual,
    };
    (gm8, meta)
}

// ============================================================
//  视频逐帧重建（← reconstructLinearHdrFrame / reconstructLinearHdrTransform）
// ============================================================

fn pam_header(width: u32, height: u32) -> Vec<u8> {
    format!("P7\nWIDTH {width}\nHEIGHT {height}\nDEPTH 3\nMAXVAL 65535\nTUPLTYPE RGB\nENDHDR\n")
        .into_bytes()
}

/// PAM 头 + 16-bit 大端像素（视频 worker 的 GPU 像素路径复用）。
pub(crate) fn pam_with_pixels(width: u32, height: u32, pixels16: &[u8]) -> Vec<u8> {
    let mut out = pam_header(width, height);
    out.extend_from_slice(pixels16);
    out
}

/// 视频链路 2（逐帧增益图）：SDR 帧 → 线性 HDR 16-bit PAM（大端 RGB）。
/// 对应 /video-frame mode=gainmap。← reconstructLinearHdrFrame (行 505)。
///
/// @param hdr_intensity_ev 高光扩展 EV（maxBoost = 2^EV，clamp 1..64）。
///   注意：此函数 **不乘** 峰值/白点上限（Kotlin 行为，与 computeGainMap 不同），
///   峰值上限由调用侧 `peak` 归一化承担。CLI 默认传 `settings.ev()`（峰值联动），
///   也可用 --hdr-intensity 显式覆盖（对应 JS 端 settings.hdrIntensity 语义）。
pub fn reconstruct_linear_hdr_frame(
    rgba: &[u8],
    width: u32,
    height: u32,
    settings: &Settings,
    peak: f64,
    hdr_intensity_ev: f64,
) -> Result<Vec<u8>> {
    let gamma = settings.gamma;
    let max_boost = 2.0f64.powf(hdr_intensity_ev).clamp(1.0, 64.0);
    let highlight_start = 0.5;
    let n = (width * height) as usize;
    let mut u16be = Vec::with_capacity(n * 6);
    for i in 0..n {
        let base = i * 4;
        let r = srgb_to_linear(rgba[base] as f64 / 255.0);
        let g = srgb_to_linear(rgba[base + 1] as f64 / 255.0);
        let b = srgb_to_linear(rgba[base + 2] as f64 / 255.0);
        let y = lum(r, g, b);
        let mask = clamp((y - highlight_start) / (1.0 - highlight_start), 0.0, 1.0).powf(gamma);
        let gain = 1.0 + (max_boost - 1.0) * mask;
        let hr = r * gain;
        let hg = g * gain;
        let hb = b * gain;
        let vr = (clamp(hr, 0.0, peak) / peak * 65535.0).round() as u16;
        let vg = (clamp(hg, 0.0, peak) / peak * 65535.0).round() as u16;
        let vb = (clamp(hb, 0.0, peak) / peak * 65535.0).round() as u16;
        u16be.extend_from_slice(&vr.to_be_bytes());
        u16be.extend_from_slice(&vg.to_be_bytes());
        u16be.extend_from_slice(&vb.to_be_bytes());
    }
    let mut out = pam_header(width, height);
    out.extend_from_slice(&u16be);
    Ok(out)
}

/// 视频直接转（单层色调映射式）逐帧重建：→ 线性 HDR 16-bit PAM。
/// 对应 /video-frame mode=transform。← reconstructLinearHdrTransform (行 593)。
/// 曝光 = peak（不再乘微调明暗）；无自动伽马（逐帧自适应会闪烁）。
pub fn reconstruct_linear_hdr_transform(
    rgba: &[u8],
    width: u32,
    height: u32,
    settings: &Settings,
    peak: f64,
) -> Result<Vec<u8>> {
    let exposure = peak;
    let gamma = settings.gamma;
    let r_adj = settings.rgb.red;
    let g_adj = settings.rgb.green;
    let b_adj = settings.rgb.blue;
    let n = (width * height) as usize;
    let mut u16be = Vec::with_capacity(n * 6);
    for i in 0..n {
        let base = i * 4;
        let r = (srgb_to_linear(rgba[base] as f64 / 255.0) * r_adj * exposure).max(0.0).powf(gamma);
        let g = (srgb_to_linear(rgba[base + 1] as f64 / 255.0) * g_adj * exposure).max(0.0).powf(gamma);
        let b = (srgb_to_linear(rgba[base + 2] as f64 / 255.0) * b_adj * exposure).max(0.0).powf(gamma);
        let vr = (clamp(r, 0.0, peak) / peak * 65535.0).round() as u16;
        let vg = (clamp(g, 0.0, peak) / peak * 65535.0).round() as u16;
        let vb = (clamp(b, 0.0, peak) / peak * 65535.0).round() as u16;
        u16be.extend_from_slice(&vr.to_be_bytes());
        u16be.extend_from_slice(&vg.to_be_bytes());
        u16be.extend_from_slice(&vb.to_be_bytes());
    }
    let mut out = pam_header(width, height);
    out.extend_from_slice(&u16be);
    Ok(out)
}

/// 视频直接转首帧预览：与视频输出完全一致的 Rec.2020/PQ 色彩管线。
/// ← videoDirectPreviewRgba (行 675)。
pub fn video_direct_preview_rgba(
    rgba: &[u8],
    width: u32,
    height: u32,
    settings: &Settings,
    peak: f64,
    white_nits: f64,
) -> Vec<u8> {
    let exposure = peak;
    let gamma = settings.gamma;
    let r_adj = settings.rgb.red;
    let g_adj = settings.rgb.green;
    let b_adj = settings.rgb.blue;
    let scale = white_nits / 10000.0;
    let n = (width * height) as usize;
    let mut out = vec![0u8; n * 4];
    for i in 0..n {
        let base = i * 4;
        let r = (srgb_to_linear(rgba[base] as f64 / 255.0) * r_adj * exposure).max(0.0).powf(gamma);
        let g = (srgb_to_linear(rgba[base + 1] as f64 / 255.0) * g_adj * exposure).max(0.0).powf(gamma);
        let b = (srgb_to_linear(rgba[base + 2] as f64 / 255.0) * b_adj * exposure).max(0.0).powf(gamma);
        let r2020 = 0.6274038959 * r + 0.3292830384 * g + 0.0433130642 * b;
        let g2020 = 0.0690972894 * r + 0.9195403951 * g + 0.0113623156 * b;
        let b2020 = 0.0163914389 * r + 0.0880133078 * g + 0.8955952528 * b;
        out[base] = (pq_encode(r2020 * scale) * 255.0).round().clamp(0.0, 255.0) as u8;
        out[base + 1] = (pq_encode(g2020 * scale) * 255.0).round().clamp(0.0, 255.0) as u8;
        out[base + 2] = (pq_encode(b2020 * scale) * 255.0).round().clamp(0.0, 255.0) as u8;
        out[base + 3] = 255;
    }
    out
}

/// PQ 编码（SMPTE ST 2084；← UltraHdrEncoder.pqEncode，行 723）。
fn pq_encode(l: f64) -> f64 {
    let ll = clamp(l, 0.0, 1.0);
    let m1 = 0.1593017578125;
    let m2 = 78.84375;
    let c1 = 0.8359375;
    let c2 = 18.8515625;
    let c3 = 18.6875;
    let lm1 = ll.powf(m1);
    ((c1 + c2 * lm1) / (1.0 + c3 * lm1)).powf(m2)
}

// ============================================================
//  自动估算 HDR 强度（← estimateHdrIntensity，行 753）
// ============================================================

/// 自动估算结果。
#[derive(Debug, Clone, Copy)]
pub struct IntensityEstimate {
    /// 建议滑块值（EV = log2(maxBoost)）
    pub hdr_intensity: f64,
    /// 建议 max_content_boost（线性倍数）
    pub max_boost: f64,
    /// 99.5 分位线性亮度（代表真实高光）
    pub y_p995: f64,
    /// 线性亮度 > 0.5 的高光像素占比
    pub hl_ratio: f64,
}

/// 基于图像亮度分布自动估算 HDR 强度（EV）。← estimateHdrIntensity。
pub fn estimate_hdr_intensity(rgba: &[u8], width: usize, height: usize) -> IntensityEstimate {
    let n = width * height;
    let mut hist = [0u32; 256];
    let mut hl_count = 0usize;
    for i in 0..n {
        let base = i * 4;
        let r = srgb_to_linear(rgba[base] as f64 / 255.0);
        let g = srgb_to_linear(rgba[base + 1] as f64 / 255.0);
        let b = srgb_to_linear(rgba[base + 2] as f64 / 255.0);
        let y = lum(r, g, b);
        if y > 0.5 {
            hl_count += 1;
        }
        let bin = ((y * 255.0) as usize).min(255);
        hist[bin] += 1;
    }
    // 99.5 分位线性亮度（从高到低累计 0.5% 像素，抗单点高亮噪声）
    let cutoff = ((n as f64 * 0.005).round() as usize).max(1);
    let mut acc = 0usize;
    let mut y_p995 = 0.0f64;
    for (bin, &count) in hist.iter().enumerate().rev() {
        acc += count as usize;
        if acc >= cutoff {
            y_p995 = (bin as f64 + 0.5) / 255.0;
            break;
        }
    }
    let hl_ratio = hl_count as f64 / n.max(1) as f64;

    let y_norm = clamp((y_p995 - 0.25) / 0.75, 0.0, 1.0);
    let mut ev = 0.8 + 0.7 * y_norm;
    if hl_ratio < 0.002 {
        ev *= 0.9;
    } else if hl_ratio > 0.02 {
        ev *= 1.05;
    }
    let hdr_intensity = ev.clamp(0.96, 3.0);
    let max_boost = 2.0f64.powf(hdr_intensity);
    IntensityEstimate {
        hdr_intensity,
        max_boost,
        y_p995,
        hl_ratio,
    }
}

// ============================================================
//  双线性下采样（← downscaleBilinear，行 794）
// ============================================================

/// 双线性下采样（单通道，8-bit）。
pub fn downscale_bilinear(
    src: &[u8],
    sw: usize,
    sh: usize,
    dw: usize,
    dh: usize,
) -> Vec<u8> {
    let mut out = vec![0u8; dw * dh];
    let xs = sw as f64 / dw as f64;
    let ys = sh as f64 / dh as f64;
    for y in 0..dh {
        let sy = y as f64 * ys;
        let y0 = (sy.floor() as usize).min(sh - 1);
        let y1 = (y0 + 1).min(sh - 1);
        let fy = sy - y0 as f64;
        for x in 0..dw {
            let sx = x as f64 * xs;
            let x0 = (sx.floor() as usize).min(sw - 1);
            let x1 = (x0 + 1).min(sw - 1);
            let fx = sx - x0 as f64;
            let v = src[y0 * sw + x0] as f64 * (1.0 - fx) * (1.0 - fy)
                + src[y0 * sw + x1] as f64 * fx * (1.0 - fy)
                + src[y1 * sw + x0] as f64 * (1.0 - fx) * fy
                + src[y1 * sw + x1] as f64 * fx * fy;
            out[y * dw + x] = v.round().clamp(0.0, 255.0) as u8;
        }
    }
    out
}

// ============================================================
//  JPEG 段构建（← UltraHdrEncoder.kt:223-299）
// ============================================================

fn build_app1_xmp(xmp: &str) -> Vec<u8> {
    let ns = b"http://ns.adobe.com/xap/1.0/\0";
    let payload = concat(&[ns, xmp.as_bytes()]);
    let mut seg = Vec::with_capacity(4 + payload.len());
    seg.extend_from_slice(&0xFFE1u16.to_be_bytes());
    seg.extend_from_slice(&((2 + payload.len()) as u16).to_be_bytes());
    seg.extend_from_slice(&payload);
    seg
}

fn build_app2_icc(icc: &[u8]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(12 + 2 + icc.len());
    payload.extend_from_slice(b"ICC_PROFILE\0");
    payload.extend_from_slice(&[1, 1]); // seq, total
    payload.extend_from_slice(icc);
    let mut seg = Vec::with_capacity(4 + payload.len());
    seg.extend_from_slice(&0xFFE2u16.to_be_bytes());
    seg.extend_from_slice(&((2 + payload.len()) as u16).to_be_bytes());
    seg.extend_from_slice(&payload);
    seg
}

/// JFIF APP0 段（与真实 Ultra HDR 文件一致；← buildJfifApp0，行 244）。
fn build_jfif_app0() -> Vec<u8> {
    vec![
        0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46, 0x49, 0x46, 0x00, 0x01, 0x01, 0x00, 0x00, 0x01, 0x00,
        0x01, 0x00, 0x00,
    ]
}

/// MPF APP2 载荷（CIPA DC-x007 标准结构，TIFF 幻数 0x002A，全部大端；← 行 250）。
fn build_mpf_payload(primary_size: u64, secondary_size: u64, secondary_offset: u64) -> Vec<u8> {
    let mut b = vec![0u8; 86];
    b[0..4].copy_from_slice(b"MPF\0");
    b[4..6].copy_from_slice(b"MM");
    b[6..8].copy_from_slice(&0x002Au16.to_be_bytes());
    b[8..12].copy_from_slice(&8u32.to_be_bytes()); // index IFD offset（相对字节4 -> 绝对12）
    b[12..14].copy_from_slice(&3u16.to_be_bytes()); // tag count
    // tag1: MPFVersion (0xB000)
    b[14..16].copy_from_slice(&0xB000u16.to_be_bytes());
    b[16..18].copy_from_slice(&0x0007u16.to_be_bytes());
    b[18..22].copy_from_slice(&4u32.to_be_bytes());
    b[22..26].copy_from_slice(b"0100");
    // tag2: NumberOfImages
    b[26..28].copy_from_slice(&0xB001u16.to_be_bytes());
    b[28..30].copy_from_slice(&0x0004u16.to_be_bytes());
    b[30..34].copy_from_slice(&1u32.to_be_bytes());
    b[34..38].copy_from_slice(&2u32.to_be_bytes());
    // tag3: MPEntry
    b[38..40].copy_from_slice(&0xB002u16.to_be_bytes());
    b[40..42].copy_from_slice(&0x0007u16.to_be_bytes());
    b[42..46].copy_from_slice(&32u32.to_be_bytes());
    b[46..50].copy_from_slice(&50u32.to_be_bytes()); // 数据偏移相对字节4 -> 绝对54
    b[50..54].copy_from_slice(&0u32.to_be_bytes()); // attribute IFD offset
    // MP Entry 0: 主图像
    b[54..58].copy_from_slice(&0x00030000u32.to_be_bytes());
    b[58..62].copy_from_slice(&(primary_size as u32).to_be_bytes());
    b[62..66].copy_from_slice(&0u32.to_be_bytes());
    // 66..70: 0x0000 0x0000 已在 vec![0] 中
    // MP Entry 1: 增益图
    b[70..74].copy_from_slice(&0u32.to_be_bytes());
    b[74..78].copy_from_slice(&(secondary_size as u32).to_be_bytes());
    b[78..82].copy_from_slice(&(secondary_offset as u32).to_be_bytes());
    // 82..86: 0x0000 0x0000
    b
}

fn build_app2_mpf(payload: &[u8]) -> Vec<u8> {
    let mut seg = Vec::with_capacity(4 + payload.len());
    seg.extend_from_slice(&0xFFE2u16.to_be_bytes());
    seg.extend_from_slice(&((2 + payload.len()) as u16).to_be_bytes());
    seg.extend_from_slice(payload);
    seg
}

// ============================================================
//  XMP 元数据（← 行 305-351）
// ============================================================

fn build_xmp_primary(gain_map_length: u64) -> String {
    format!(
        "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"XMP Core 5.5.0\">\n\
         <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
          <rdf:Description\n\
           xmlns:Container=\"http://ns.google.com/photos/1.0/container/\"\n\
           xmlns:Item=\"http://ns.google.com/photos/1.0/container/item/\"\n\
           xmlns:hdrgm=\"http://ns.adobe.com/hdr-gain-map/1.0/\"\n\
           hdrgm:Version=\"1.0\">\n\
           <Container:Directory>\n\
            <rdf:Seq>\n\
             <rdf:li rdf:parseType=\"Resource\">\n\
              <Container:Item Item:Semantic=\"Primary\" Item:Mime=\"image/jpeg\"/>\n\
             </rdf:li>\n\
             <rdf:li rdf:parseType=\"Resource\">\n\
              <Container:Item Item:Semantic=\"GainMap\" Item:Mime=\"image/jpeg\" Item:Length=\"{gain_map_length}\"/>\n\
             </rdf:li>\n\
            </rdf:Seq>\n\
           </Container:Directory>\n\
          </rdf:Description>\n\
         </rdf:RDF>\n\
        </x:xmpmeta>\n"
    )
}

fn build_xmp_secondary(meta: &GainMapMetadata) -> String {
    let gm_min = meta.min_content_boost.log2();
    let gm_max = meta.max_content_boost.log2();
    let hc_min = meta.hdr_capacity_min.log2();
    let hc_max = meta.hdr_capacity_max.log2();
    format!(
        "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"XMP Core 5.5.0\">\n\
         <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
          <rdf:Description rdf:about=\"\"\n\
           xmlns:hdrgm=\"http://ns.adobe.com/hdr-gain-map/1.0/\"\n\
           hdrgm:Version=\"1.0\"\n\
           hdrgm:GainMapMin=\"{}\"\n\
           hdrgm:GainMapMax=\"{}\"\n\
           hdrgm:Gamma=\"{}\"\n\
           hdrgm:OffsetSDR=\"{}\"\n\
           hdrgm:OffsetHDR=\"{}\"\n\
           hdrgm:HDRCapacityMin=\"{}\"\n\
           hdrgm:HDRCapacityMax=\"{}\"\n\
           hdrgm:BaseRenditionIsHDR=\"False\">\n\
          </rdf:Description>\n\
         </rdf:RDF>\n\
        </x:xmpmeta>\n",
        fmt_xmp(gm_min),
        fmt_xmp(gm_max),
        fmt_xmp(meta.gamma),
        fmt_xmp(meta.offset_sdr),
        fmt_xmp(meta.offset_hdr),
        fmt_xmp(hc_min),
        fmt_xmp(hc_max),
    )
}

// ============================================================
//  JPEG 编码 / 重组（← 行 823-931）
// ============================================================

fn encode_jpeg_rgb(rgba: &[u8], width: u32, height: u32, quality: f64) -> Result<Vec<u8>> {
    let q = ((quality.clamp(0.1, 1.0)) * 100.0).round().clamp(1.0, 100.0) as u8;
    let n = (width as usize) * (height as usize);
    let mut rgb = Vec::with_capacity(n * 3);
    for i in 0..n {
        rgb.extend_from_slice(&rgba[i * 4..i * 4 + 3]);
    }
    let mut buf = Vec::new();
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, q);
    enc.encode(&rgb, width, height, image::ExtendedColorType::Rgb8)
        .context("主图像 JPEG 编码失败")?;
    Ok(buf)
}

fn encode_jpeg_gray(gray: &[u8], width: u32, height: u32, quality: f64) -> Result<Vec<u8>> {
    let q = ((quality.clamp(0.1, 1.0)) * 100.0).round().clamp(1.0, 100.0) as u8;
    let mut buf = Vec::new();
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, q);
    enc.encode(gray, width, height, image::ExtendedColorType::L8)
        .context("增益图 JPEG 编码失败")?;
    Ok(buf)
}

/// 去除 JPEG 开头的 APPn/COM 段，返回 body（DQT/SOF/DHT/SOS...）；← stripJpegAppSegments。
fn strip_jpeg_app_segments(jpeg: &[u8]) -> Vec<u8> {
    let mut off = 2usize;
    while off + 4 <= jpeg.len() {
        if jpeg[off] != 0xFF {
            break;
        }
        let marker = jpeg[off + 1];
        if (0xE0..=0xEF).contains(&marker) || marker == 0xFE {
            off += 2 + read_u16(jpeg, off + 2) as usize;
        } else {
            break;
        }
    }
    jpeg[off..].to_vec()
}

/// 重组主图像：SOI + APP0 + APP1(XMP) + APP2(ICC) + DQT/SOF/DHT + APP2(MPF) + SOS+data+EOI。
/// 返回 (无 MPF 的缓冲, MPF 插入位置)；← reorderPrimary (行 873)。
fn reorder_primary(
    primary_jpeg: &[u8],
    app1_xmp: &[u8],
    app2_icc: &[u8],
) -> (Vec<u8>, usize) {
    let mut head_app: Vec<Vec<u8>> = Vec::new();
    let mut off = 2usize;
    while off + 4 <= primary_jpeg.len() {
        if primary_jpeg[off] != 0xFF {
            break;
        }
        let marker = primary_jpeg[off + 1];
        if (0xE0..=0xEF).contains(&marker) {
            let len = read_u16(primary_jpeg, off + 2) as usize;
            head_app.push(primary_jpeg[off..off + 2 + len].to_vec());
            off += 2 + len;
        } else {
            break;
        }
    }

    let body_start = off;
    let mut p = off;
    let mut sos_offset: Option<usize> = None;
    while p + 4 <= primary_jpeg.len() {
        if primary_jpeg[p] != 0xFF {
            p += 1;
            continue;
        }
        let marker = primary_jpeg[p + 1];
        if marker == 0xFF {
            p += 1;
            continue;
        }
        if (0xD0..=0xD7).contains(&marker) {
            p += 2;
            continue;
        }
        if marker == 0xDA {
            sos_offset = Some(p);
            break;
        }
        if marker == 0xD9 {
            break;
        }
        p += 2 + read_u16(primary_jpeg, p + 2) as usize;
    }

    let before_sos = match sos_offset {
        Some(so) => primary_jpeg[body_start..so].to_vec(),
        None => primary_jpeg[body_start..].to_vec(),
    };
    let sos_and_rest = match sos_offset {
        Some(so) => primary_jpeg[so..].to_vec(),
        None => Vec::new(),
    };

    // 确保开头恰好一个 JFIF APP0（与真实 Ultra HDR 文件一致）
    let non_jfif: Vec<Vec<u8>> = head_app
        .into_iter()
        .filter(|seg| !(seg.len() >= 6 && seg[4] == 0x4A && seg[5] == 0x46))
        .collect();
    let mut head: Vec<Vec<u8>> = vec![build_jfif_app0()];
    head.extend(non_jfif);
    let head_len: usize = head.iter().map(|s| s.len()).sum();
    let pos_before_mpf = 2 + head_len + app1_xmp.len() + app2_icc.len() + before_sos.len();

    let mut out = Vec::with_capacity(2 + head_len + app1_xmp.len() + app2_icc.len()
        + before_sos.len() + sos_and_rest.len());
    out.extend_from_slice(&primary_jpeg[..2]); // SOI
    for h in &head {
        out.extend_from_slice(h);
    }
    out.extend_from_slice(app1_xmp);
    out.extend_from_slice(app2_icc);
    out.extend_from_slice(&before_sos);
    out.extend_from_slice(&sos_and_rest);
    (out, pos_before_mpf)
}

// ============================================================
//  sRGB ICC 配置文件（程序化生成；← buildSrgbIcc，行 143）
// ============================================================

/// 构建 sRGB ICC v4 profile（对应 Kotlin `buildSrgbIcc`，行 143）。
///
/// ⚠️ 有意分歧：Kotlin 该函数**漏写标签签名**（tag table 只写 offset/size，产出无效 ICC；
/// 且 encode 路径实际用的是内嵌 base64 ICC，此函数在 Kotlin 侧从未被使用）。本实现补齐
/// 签名，产出结构有效的 ICC；其余字段（header/曲线/主色/排序）与 Kotlin 逐位一致。
pub fn build_srgb_icc() -> Vec<u8> {
    fn para_type4(g: f64, a: f64, b: f64, c: f64, d: f64) -> Vec<u8> {
        let mut buf = vec![0u8; 32];
        buf[0..4].copy_from_slice(b"para");
        buf[8..10].copy_from_slice(&4u16.to_be_bytes());
        buf[12..16].copy_from_slice(&(s15(g) as u32).to_be_bytes());
        buf[16..20].copy_from_slice(&(s15(a) as u32).to_be_bytes());
        buf[20..24].copy_from_slice(&(s15(b) as u32).to_be_bytes());
        buf[24..28].copy_from_slice(&(s15(c) as u32).to_be_bytes());
        buf[28..32].copy_from_slice(&(s15(d) as u32).to_be_bytes());
        buf
    }
    fn xyz_type(x: f64, y: f64, z: f64) -> Vec<u8> {
        let mut buf = vec![0u8; 20];
        buf[0..4].copy_from_slice(b"XYZ ");
        buf[8..12].copy_from_slice(&(s15(x) as u32).to_be_bytes());
        buf[12..16].copy_from_slice(&(s15(y) as u32).to_be_bytes());
        buf[16..20].copy_from_slice(&(s15(z) as u32).to_be_bytes());
        buf
    }
    fn text_type(sig: &[u8; 4], s: &str) -> Vec<u8> {
        let bytes = s.as_bytes();
        let mut buf = vec![0u8; 12 + bytes.len()];
        buf[0..4].copy_from_slice(sig);
        buf[8..12].copy_from_slice(&(bytes.len() as u32).to_be_bytes());
        buf[12..].copy_from_slice(bytes);
        buf
    }

    let srg = para_type4(2.4, 1.0 / 1.055, 0.055 / 1.055, 1.0 / 12.92, 0.04045);
    // 按签名排序（ICC v4 要求）
    let mut tags: Vec<(&str, Vec<u8>)> = vec![
        ("bTRC", srg.clone()),
        ("bXYZ", xyz_type(0.143051, 0.060608, 0.713913)),
        ("cprt", text_type(b"text", "Public Domain")),
        ("desc", text_type(b"desc", "sRGB")),
        ("gTRC", srg.clone()),
        ("gXYZ", xyz_type(0.385113, 0.716879, 0.097109)),
        ("rTRC", srg),
        ("rXYZ", xyz_type(0.436041, 0.222485, 0.01392)),
        ("wtpt", xyz_type(0.9642, 1.0, 0.8249)),
    ];
    tags.sort_by(|a, b| a.0.cmp(b.0));

    let mut header = vec![0u8; 128];
    header[4..8].copy_from_slice(b"lcms");
    header[8..12].copy_from_slice(&0x04300000u32.to_be_bytes()); // ICC v4.3
    header[12..16].copy_from_slice(b"mntr");
    header[16..20].copy_from_slice(b"RGB ");
    header[20..24].copy_from_slice(b"XYZ ");
    header[36..40].copy_from_slice(b"acsp");
    header[40..44].copy_from_slice(b"MSFT");
    header[64..68].copy_from_slice(&1u32.to_be_bytes()); // rendering intent = 1
    header[68..72].copy_from_slice(&(s15(0.9642) as u32).to_be_bytes());
    header[72..76].copy_from_slice(&(s15(1.0) as u32).to_be_bytes());
    header[76..80].copy_from_slice(&(s15(0.8249) as u32).to_be_bytes());
    header[80..84].copy_from_slice(b"lcms");

    let tag_table_start = 128usize;
    let tag_table_size = 4 + tags.len() * 12;
    let mut tag_table = vec![0u8; tag_table_size];
    tag_table[0..4].copy_from_slice(&(tags.len() as u32).to_be_bytes());

    let mut chunks = Vec::new();
    let mut offset = tag_table_start + tag_table_size;
    for (i, (name, data)) in tags.iter().enumerate() {
        tag_table[4 + i * 12..8 + i * 12].copy_from_slice(name.as_bytes());
        tag_table[8 + i * 12..12 + i * 12].copy_from_slice(&(offset as u32).to_be_bytes());
        tag_table[12 + i * 12..16 + i * 12].copy_from_slice(&(data.len() as u32).to_be_bytes());
        chunks.extend_from_slice(data);
        offset += data.len();
    }

    let mut profile = concat(&[&header, &tag_table, &chunks]);
    let size = profile.len() as u32;
    profile[0..4].copy_from_slice(&size.to_be_bytes());
    profile
}

// ============================================================
//  总入口（← encode，行 954）
// ============================================================

/// 将图像编码为 Ultra HDR JPEG。
///
/// 主图策略（依据规范："SDR 图像的色彩配置定义了 HDR 图像的色彩空间"）：
/// - `primary_srgb=true` → 主图保持原始 sRGB 像素 + sRGB ICC（任何查看器看到原图）
/// - 检测到输入为 Display-P3 → 主图已是 P3 像素，保持 + P3 ICC
/// - 其他（sRGB / 未声明）→ sRGB 像素转 Display-P3 + P3 ICC（与 Google 文件一致）
pub fn encode_ultra_hdr(
    primary_rgba: &[u8],
    width: u32,
    height: u32,
    settings: &Settings,
    detected_cs: Option<InputColorSpace>,
) -> Result<Vec<u8>> {
    let gain_map_scale = 4u32;
    let main_rgba = if settings.primary_srgb || detected_cs == Some(InputColorSpace::DisplayP3) {
        primary_rgba.to_vec()
    } else {
        // GPU 优先（HDRCONV_GPU=1），失败回退 CPU
        crate::gpu::try_gpu_srgb_to_p3(primary_rgba, width, height)
            .unwrap_or_else(|| srgb_rgba_to_display_p3_rgba(primary_rgba, width as usize, height as usize))
    };
    let primary_icc = if settings.primary_srgb {
        uhdr_gainmap_icc().to_vec()
    } else {
        uhdr_primary_icc().to_vec()
    };

    // 1. 增益图（SDR 基准 = mainRgba 的线性亮度，即主图色彩空间；GPU 优先）
    let (gm8, meta) = match crate::gpu::try_gpu_compute_gainmap(
        &main_rgba,
        width,
        height,
        settings.gain_ev(),
        settings.gamma,
    ) {
        Some((gm, min_b, max_b)) => (
            gm,
            GainMapMetadata {
                min_content_boost: min_b,
                max_content_boost: max_b,
                gamma: 1.0,
                offset_sdr: 1.0 / 64.0,
                offset_hdr: 1.0 / 64.0,
                hdr_capacity_min: 1.0,
                hdr_capacity_max: max_b,
            },
        ),
        None => compute_gain_map(&main_rgba, width as usize, height as usize, settings),
    };
    let gm_w = (width / gain_map_scale).max(1);
    let gm_h = (height / gain_map_scale).max(1);
    let down = downscale_bilinear(
        &gm8,
        width as usize,
        height as usize,
        gm_w as usize,
        gm_h as usize,
    );

    // 2. 增益图灰度基线 JPEG
    let quality = settings.quality.clamp(0.1, 1.0);
    let gm_jpeg = encode_jpeg_gray(&down, gm_w, gm_h, quality)?;

    // 3. 次图像: 独立 JPEG = SOI + APP0(JFIF) + APP1(hdrgm XMP) + APP2(ICC) + body
    let mut secondary = Vec::new();
    secondary.extend_from_slice(&[0xFF, 0xD8]);
    secondary.extend_from_slice(&build_jfif_app0());
    secondary.extend_from_slice(&build_app1_xmp(&build_xmp_secondary(&meta)));
    secondary.extend_from_slice(&build_app2_icc(uhdr_gainmap_icc()));
    secondary.extend_from_slice(&strip_jpeg_app_segments(&gm_jpeg));
    let secondary_size = secondary.len() as u64;

    // 4. 主图像 JPEG（基线，像素 = mainRgba）
    let primary_jpeg = encode_jpeg_rgb(&main_rgba, width, height, quality)?;

    // 5. 主图像 XMP + ICC
    let app1_xmp = build_app1_xmp(&build_xmp_primary(secondary_size));
    let app2_icc = build_app2_icc(&primary_icc);

    // 6. 重组主图像 + 计算 MPF 偏移
    let (buf_no_mpf, pos_before_mpf) = reorder_primary(&primary_jpeg, &app1_xmp, &app2_icc);
    let mpf_payload_len = 86u64;
    let mpf_app2_len = 4 + mpf_payload_len;
    let sos_and_rest_len = (buf_no_mpf.len() - pos_before_mpf) as u64;
    let primary_size = pos_before_mpf as u64 + mpf_app2_len + sos_and_rest_len;
    let secondary_offset = primary_size - pos_before_mpf as u64 - 8;

    let mpf_app2 = build_app2_mpf(&build_mpf_payload(
        primary_size,
        secondary_size,
        secondary_offset,
    ));
    let mut primary = Vec::with_capacity(buf_no_mpf.len() + mpf_app2.len());
    primary.extend_from_slice(&buf_no_mpf[..pos_before_mpf]);
    primary.extend_from_slice(&mpf_app2);
    primary.extend_from_slice(&buf_no_mpf[pos_before_mpf..]);

    // 7. 拼接：主图 + 增益图
    primary.extend_from_slice(&secondary);
    Ok(primary)
}