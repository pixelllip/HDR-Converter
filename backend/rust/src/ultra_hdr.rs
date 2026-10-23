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

use crate::colorspace::{InputCodec, InputColorSpace};
use crate::models::Settings;

/// 增益图相对主图的分辨率分母：1 = 全分辨率（峰值精确），4 = 1/4（体积小，但双线性上采样会稀释峰值）。
pub const GAINMAP_SCALE: usize = 1;

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
//  增益图生成（← computeGainMap，行 369）
// ============================================================

//  增益曲线（B 方案，唯一实现）：明度曲线 + 白点滚降 + 局部镜面高光
//
//  `tone_target` / `tone_gain` 同时被 ICC 路径（convert.rs）和 Ultra HDR 增益图使用，
//  白点滚降系数 `settings.rolloff` 是两者共用的用户参数。局部镜面高光需要邻域信息，
//  仅 Ultra HDR 增益图（compute_gain_map）使用；ICC 路径只走全局明度曲线。
// ============================================================

const ROLLOFF_PEAK_Y: f64 = 0.30;
const ROLLOFF_PEAK_FRAC: f64 = 0.80;
const ROLLOFF_SPEC_RADIUS: usize = 3;
const ROLLOFF_GATE_LO: f64 = 0.70;
const ROLLOFF_GATE_HI: f64 = 0.95;

#[inline]
fn smoothstep(a: f64, b: f64, x: f64) -> f64 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// 共用 tone 曲线（ICC 与 Ultra HDR）：`y^gamma` → 中调峰值 0.8M、白点 `rolloff*M` 滚降。
/// `gamma`（`settings.image_gamma`，0.1~2）：**亮度域**伽马，只改亮度曲线形状，色度不变
/// （对 RGB 乘同一增益）；`y=1` 时 `1^γ=1`，所以白点不受伽马影响——白点端只由 `rolloff` 控制。
/// `rolloff=1` 时白点到峰值（无滚降）；`rolloff=0` 时白点完全不提亮。
pub fn tone_target(y: f64, m: f64, rolloff: f64, gamma: f64) -> f64 {
    let y = y.clamp(0.0, 1.0).powf(gamma.clamp(0.1, 2.0));
    let m = m.max(1.0);
    if m <= 1.0 {
        return y; // 无曝光（峰值 ≤ 白点）→ 恒等，避免滚降曲线在 m=1 时仍改变中调
    }
    let r = rolloff.clamp(0.0, 1.0);
    let t_peak = ROLLOFF_PEAK_Y * (1.0 + (m - 1.0) * ROLLOFF_PEAK_FRAC);
    if y <= ROLLOFF_PEAK_Y {
        y * (1.0 + (m - 1.0) * ROLLOFF_PEAK_FRAC * smoothstep(0.0, ROLLOFF_PEAK_Y, y))
    } else {
        t_peak + ((1.0 + (m - 1.0) * r) - t_peak) * smoothstep(ROLLOFF_PEAK_Y, 1.0, y)
    }
}

pub fn tone_gain(y: f64, m: f64, rolloff: f64, gamma: f64) -> f64 {
    if y <= 0.0 {
        1.0
    } else {
        // 分母用**原** y：输出 = gain·y = tone_target(y^γ)
        tone_target(y, m, rolloff, gamma) / y
    }
}

/// 峰值锚定的增益倍数：画面最亮点 `y_max > 0.5` 时把该点推向曝光（`m = 曝光 / y_max`），
/// 否则视为低动态范围图，直接使用曝光。Ultra HDR 增益图与 ICC 路径的 `m` 都按此语义取。
#[inline]
pub fn tone_exposure(exposure: f64, y_max: f64) -> f64 {
    if y_max > 0.5 {
        (exposure / y_max).max(1.0)
    } else {
        exposure.max(1.0)
    }
}

fn morph_1d(src: &[f64], len: usize, r: usize, want_max: bool) -> Vec<f64> {
    let mut out = vec![0.0; len];
    for (i, o) in out.iter_mut().enumerate() {
        let lo = i.saturating_sub(r);
        let hi = (i + r).min(len.saturating_sub(1));
        let mut v = src[lo];
        for &x in &src[lo..=hi] {
            v = if want_max { v.max(x) } else { v.min(x) };
        }
        *o = v;
    }
    out
}

fn morph_2d(src: &[f64], w: usize, h: usize, r: usize, want_max: bool) -> Vec<f64> {
    let mut rows = vec![0.0; w * h];
    rows.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let src_row = &src[y * w..(y + 1) * w];
        let f = morph_1d(src_row, w, r, want_max);
        row.copy_from_slice(&f);
    });
    let mut out = vec![0.0; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            let lo = y.saturating_sub(r);
            let hi = (y + r).min(h.saturating_sub(1));
            let mut v = rows[lo * w + x];
            for yy in lo..=hi {
                let z = rows[yy * w + x];
                v = if want_max { v.max(z) } else { v.min(z) };
            }
            row[x] = v;
        }
    });
    out
}

/// 局部镜面高光权重：top-hat（开运算）区分小面积亮斑与大面积白。
fn rolloff_specular(y: &[f64], w: usize, h: usize) -> Vec<f64> {
    let r = ROLLOFF_SPEC_RADIUS
        .min(w.min(h).saturating_sub(1) / 2)
        .max(1);
    let eroded = morph_2d(y, w, h, r, false);
    let opened = morph_2d(&eroded, w, h, r, true);
    (0..w * h)
        .map(|i| {
            let yv = y[i];
            let ov = opened[i];
            let s = ((yv - ov) / (1.0 - ov).max(1e-3)).clamp(0.0, 1.0);
            (s * smoothstep(ROLLOFF_GATE_LO, ROLLOFF_GATE_HI, yv)).clamp(0.0, 1.0)
        })
        .collect()
}

/// 生成增益图（B 方案：明度曲线 + 白点滚降 + 局部镜面高光）。
///
/// - 全局部分走 `tone_target` / `tone_gain`（与 ICC 路径同一函数），`m` 由画面最亮点锚定；
/// - 白点按 `settings.rolloff` 滚降（1=白点到峰值，0=白点不提亮）；
/// - 局部镜面高光用 top-hat 检出小面积亮斑，推到峰值（全局曲线照顾不到的镜面反光）；
/// - 增益图全分辨率（`GAINMAP_SCALE=1`），避免双线性上采样稀释峰值。
///
/// 返回 (增益图 8-bit, 元数据)。
pub fn compute_gain_map(
    primary_rgba: &[u8],
    width: usize,
    height: usize,
    settings: &Settings,
) -> (Vec<u8>, GainMapMetadata) {
    let exposure = (settings.peak_nits / settings.white_nits).max(1.0);
    let offset = 1.0 / 64.0;
    let codec = InputCodec::from_settings(settings);

    let gain_scale = settings.gain_scale.max(1) as usize;
    let gm_w = (width / gain_scale).max(1);
    let gm_h = (height / gain_scale).max(1);
    let n = gm_w * gm_h;
    let low_rgba = downscale_area_average_box_rgba(primary_rgba, width, height, gm_w, gm_h);
    let y_low: Vec<f64> = (0..n)
        .map(|i| {
            let base = i * 4;
            let (r, g, b) = codec.to_linear(
                low_rgba[base] as f64 / 255.0,
                low_rgba[base + 1] as f64 / 255.0,
                low_rgba[base + 2] as f64 / 255.0,
            );
            lum(r, g, b)
        })
        .collect();

    // 峰值锚定：画面最亮点越亮，m 越大；低动态范围图直接用曝光。
    let y_max = y_low.iter().copied().fold(0.0f64, f64::max).max(1e-6);
    let m_eff = tone_exposure(exposure, y_max);

    let mut spec = rolloff_specular(&y_low, gm_w, gm_h);
    if let Some((idx, &peak)) = y_low
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
    {
        if peak > 0.5 && spec[idx] > 0.05 {
            spec[idx] = 1.0;
        }
    }
    let gain: Vec<f64> = y_low
        .iter()
        .enumerate()
        .map(|(i, &y)| {
            let gt = tone_gain(y, m_eff, settings.rolloff, settings.image_gamma);
            let g = (gt + (m_eff - gt) * spec[i]).min(m_eff).max(1.0);
            let t = (y * g).min(exposure);
            (t + offset) / (y + offset)
        })
        .collect();
    let gmin = gain.iter().copied().fold(f64::INFINITY, f64::min);
    let gmax = gain.iter().copied().fold(f64::NEG_INFINITY, f64::max);

    let min_boost = gmin.min(1.0).clamp(0.25, 1.0); // Kotlin: clamp(Math.min(1.0, gmin), 0.25, 1.0)
    let max_boost_actual = gmax.max(1.0);
    let map_min = min_boost.log2();
    let map_max = max_boost_actual.log2();
    let range = (map_max - map_min).max(1e-6);

    // 直接量化到 8-bit（无需再下采样）
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
        // 显示 headroom = 峰值/白点（峰值亮度）；max_content_boost 是最大增益（中调），
        // 二者不同：capacity 控制显示端 w，保证 1250 nit 屏上 w=1、峰值不被压。
        hdr_capacity_max: exposure,
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
    let codec = InputCodec::from_settings(settings);
    let mut u16be = Vec::with_capacity(n * 6);
    for i in 0..n {
        let base = i * 4;
        let (lr, lg, lb) = codec.to_linear(
            rgba[base] as f64 / 255.0,
            rgba[base + 1] as f64 / 255.0,
            rgba[base + 2] as f64 / 255.0,
        );
        let r = (lr * r_adj * exposure).max(0.0).powf(gamma);
        let g = (lg * g_adj * exposure).max(0.0).powf(gamma);
        let b = (lb * b_adj * exposure).max(0.0).powf(gamma);
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
    let codec = InputCodec::from_settings(settings);
    let mut out = vec![0u8; n * 4];
    for i in 0..n {
        let base = i * 4;
        let (lr, lg, lb) = codec.to_linear(
            rgba[base] as f64 / 255.0,
            rgba[base + 1] as f64 / 255.0,
            rgba[base + 2] as f64 / 255.0,
        );
        let r = (lr * r_adj * exposure).max(0.0).powf(gamma);
        let g = (lg * g_adj * exposure).max(0.0).powf(gamma);
        let b = (lb * b_adj * exposure).max(0.0).powf(gamma);
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
//  双线性下采样（← downscaleBilinear，行 794）
// ============================================================

/// 双线性下采样（单通道，8-bit）。
///
/// ⚠️ 历史实现：对 ~4× 下采样只用 2×2 双线性 tap（decimation），相当于"跳过 3/4 的
/// 像素 + 双线性内插"，**缺乏低通预滤波**，会在锐利阶跃处产生混叠/锐利残差，进入
/// JPEG 后再被 DCT 振铃放大 → 解码端 4× 上采样后呈"毛边/光晕"。
///
/// 新链路见 `downscale_area_average_box`：先做面积平均再抽样，对锐利阶跃得到单调
/// 平滑过渡，避免混叠。本函数保留供回归对比使用，不在 Ultra HDR 主链路调用。
pub fn downscale_bilinear(src: &[u8], sw: usize, sh: usize, dw: usize, dh: usize) -> Vec<u8> {
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

/// 面积平均（box）下采样（单通道，8-bit），对齐 Android 官方建议：
/// Ultra HDR 增益图应在编码前下采样到主图每边 ¼，对应的"低通预滤波 + 抽样"
/// 实现。对每个目标像素取源上对应 SxT 矩形（自适应非整除）像素的算术平均，
/// 量化到 8-bit 输出。
///
/// 设计动机（详见文件头注释）：
/// - 全分辨率逐像素 mask/gain 在物体高光边界处有 1px 锐阶跃；
/// - 用 2×2 双线性 decimation 抽 4× 缺乏低通，锐阶跃作为高频信号混入 1/4 增益图，
///   再经 JPEG DCT 振铃 → 解码端 4× 上采样后呈"毛边/光晕"；
/// - 面积平均保证锐阶跃被平均到一个**单调、平滑**的过渡带，过渡带宽度与下采样
///   核（约 4 源像素）匹配，与解码端上采样尺度一致。
///
/// 与 Kotlin UltraHdrEncoder.downscaleBilinear 的有意差异：Kotlin 侧 2×2 bilinear
/// decimation（保留用于回归对照），本实现为 box-average 主链路。
pub fn downscale_area_average_box(
    src: &[u8],
    sw: usize,
    sh: usize,
    dw: usize,
    dh: usize,
) -> Vec<u8> {
    assert!(dw > 0 && dh > 0, "目标尺寸必须 > 0");
    assert!(
        dw <= sw && dh <= sh,
        "box 平均仅支持下采样（dw<=sw, dh<=sh），当前 sw={sw} sh={sh} dw={dw} dh={dh}"
    );
    let mut out = vec![0u8; dw * dh];
    let xs = sw as f64 / dw as f64;
    let ys = sh as f64 / dh as f64;
    for y in 0..dh {
        let sy0 = (y as f64 * ys).floor() as usize;
        let sy1 = ((y as f64 + 1.0) * ys).ceil() as usize;
        let y0 = sy0.min(sh);
        let y1 = sy1.min(sh).max(y0 + 1);
        for x in 0..dw {
            let sx0 = (x as f64 * xs).floor() as usize;
            let sx1 = ((x as f64 + 1.0) * xs).ceil() as usize;
            let x0 = sx0.min(sw);
            let x1 = sx1.min(sw).max(x0 + 1);
            // 累加 [y0..y1) × [x0..x1) 矩形像素
            let mut sum: u32 = 0;
            let mut count: u32 = 0;
            for yy in y0..y1 {
                let row = yy * sw;
                for xx in x0..x1 {
                    sum += src[row + xx] as u32;
                    count += 1;
                }
            }
            let v = sum as f64 / count as f64;
            out[y * dw + x] = v.round().clamp(0.0, 255.0) as u8;
        }
    }
    out
}

/// 面积平均（box）下采样 RGBA（4 通道，8-bit/通道）。每个通道独立 box 平均。
///
/// 用途：`compute_gain_map` 第 0 步把主图下采样到增益图分辨率（约主图每边 ¼），
/// 让 mask/gain 直接在低分辨率上计算——避免全分辨率逐像素锐阶跃进入增益图。
pub fn downscale_area_average_box_rgba(
    src: &[u8],
    sw: usize,
    sh: usize,
    dw: usize,
    dh: usize,
) -> Vec<u8> {
    assert!(dw > 0 && dh > 0, "目标尺寸必须 > 0");
    assert!(
        dw <= sw && dh <= sh,
        "box 平均仅支持下采样（dw<=sw, dh<=sh），当前 sw={sw} sh={sh} dw={dw} dh={dh}"
    );
    assert!(src.len() >= sw * sh * 4, "src 字节数不足以填充 sw×sh×4");
    let mut out = vec![0u8; dw * dh * 4];
    let xs = sw as f64 / dw as f64;
    let ys = sh as f64 / dh as f64;
    for y in 0..dh {
        let sy0 = (y as f64 * ys).floor() as usize;
        let sy1 = ((y as f64 + 1.0) * ys).ceil() as usize;
        let y0 = sy0.min(sh);
        let y1 = sy1.min(sh).max(y0 + 1);
        for x in 0..dw {
            let sx0 = (x as f64 * xs).floor() as usize;
            let sx1 = ((x as f64 + 1.0) * xs).ceil() as usize;
            let x0 = sx0.min(sw);
            let x1 = sx1.min(sw).max(x0 + 1);
            let mut sums = [0u32; 4];
            let mut count: u32 = 0;
            for yy in y0..y1 {
                let row = yy * sw;
                for xx in x0..x1 {
                    let base = (row + xx) * 4;
                    sums[0] += src[base] as u32;
                    sums[1] += src[base + 1] as u32;
                    sums[2] += src[base + 2] as u32;
                    sums[3] += src[base + 3] as u32;
                    count += 1;
                }
            }
            let inv = 1.0 / count as f64;
            let out_base = (y * dw + x) * 4;
            for c in 0..4 {
                out[out_base + c] = (sums[c] as f64 * inv).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    out
}

/// 3×3 高斯模糊（等价 sigma≈1），分离卷积实现：1×3 盒式 + 3×1 盒式各一次。
///
/// 输入值域 [0, 1]（如 gain mask），输出仍在 [0, 1] 内（正系数 + 归一化核）。
/// 单调非降的 1D 序列沿任何方向经此核卷积仍单调非降（凸/单调保持）——
/// 因此把"硬阈值"mask（如 `clamp((y-0.5)/0.5, 0, 1)`）卷积后得到"软阈值"mask，
/// 阶跃从 1 个低分辨率像素展宽到 2~3 个低分辨率像素，且无过冲。
///
/// 边界采用 clamp-to-edge（越界像素取最边缘值）。
pub fn gaussian_blur_33(src: &[f64], w: usize, h: usize) -> Vec<f64> {
    assert_eq!(src.len(), w * h);
    // 水平 1×3 盒式 (1, 1, 1) / 3
    let mut tmp = vec![0.0f64; w * h];
    for y in 0..h {
        for x in 0..w {
            let xm1 = if x == 0 { 0 } else { x - 1 };
            let xp1 = if x + 1 >= w { w - 1 } else { x + 1 };
            tmp[y * w + x] = (src[y * w + xm1] + src[y * w + x] + src[y * w + xp1]) / 3.0;
        }
    }
    // 垂直 3×1 盒式 (1, 1, 1) / 3
    let mut out = vec![0.0f64; w * h];
    for y in 0..h {
        let ym1 = if y == 0 { 0 } else { y - 1 };
        let yp1 = if y + 1 >= h { h - 1 } else { y + 1 };
        for x in 0..w {
            out[y * w + x] = (tmp[ym1 * w + x] + tmp[y * w + x] + tmp[yp1 * w + x]) / 3.0;
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
    let q = ((quality.clamp(0.1, 1.0)) * 100.0)
        .round()
        .clamp(1.0, 100.0) as u8;
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
    let q = ((quality.clamp(0.1, 1.0)) * 100.0)
        .round()
        .clamp(1.0, 100.0) as u8;
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
fn reorder_primary(primary_jpeg: &[u8], app1_xmp: &[u8], app2_icc: &[u8]) -> (Vec<u8>, usize) {
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

    let mut out = Vec::with_capacity(
        2 + head_len + app1_xmp.len() + app2_icc.len() + before_sos.len() + sos_and_rest.len(),
    );
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
//  通用 RGB ICC 生成器（原汤化原食：按输入色彩空间生成主图 ICC）
// ============================================================

/// 幂函数 TRC：Y = X^g（ICC para type 0；Adobe RGB gamma 2.2、DCI-P3 gamma 2.6、
/// ProPhoto gamma 1.8、Rec.2020 常用 gamma 近似均可用）。sRGB 用专门的分段曲线（type 4）。
fn para_type0(g: f64) -> Vec<u8> {
    let mut buf = vec![0u8; 32];
    buf[0..4].copy_from_slice(b"para");
    buf[8..10].copy_from_slice(&0u16.to_be_bytes()); // function type = 0（幂函数）
    buf[12..16].copy_from_slice(&(s15(g) as u32).to_be_bytes());
    buf
}

/// 生成 RGB 显示 profile（ICC v4.3），按主色 + TRC 参数化（结构/排序与 build_srgb_icc 一致）。
/// `cicp_primaries`：Some(primaries) 时附加 `cicp` 标签（transfer=16 PQ、matrix=0、full range=1）。
fn build_rgb_icc(
    name: &str,
    r_xyz: [f64; 3],
    g_xyz: [f64; 3],
    b_xyz: [f64; 3],
    trc: Vec<u8>,
    cicp_primaries: Option<u8>,
) -> Vec<u8> {
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
    fn cicp_type(primaries: u8) -> Vec<u8> {
        // ICC `cicp`：4 字节类型 + 4 字节保留 + primaries/transfer/matrix/full-range
        let mut buf = vec![0u8; 12];
        buf[0..4].copy_from_slice(b"cicp");
        buf[8] = primaries;
        buf[9] = 16; // transfer characteristics = PQ
        buf[10] = 0; // matrix coefficients = identity
        buf[11] = 1; // full range
        buf
    }

    let mut tags: Vec<(&str, Vec<u8>)> = vec![
        ("bTRC", trc.clone()),
        ("bXYZ", xyz_type(b_xyz[0], b_xyz[1], b_xyz[2])),
        ("cprt", text_type(b"text", "Public Domain")),
        ("desc", text_type(b"desc", name)),
        ("gTRC", trc.clone()),
        ("gXYZ", xyz_type(g_xyz[0], g_xyz[1], g_xyz[2])),
        ("rTRC", trc),
        ("rXYZ", xyz_type(r_xyz[0], r_xyz[1], r_xyz[2])),
        ("wtpt", xyz_type(0.9642, 1.0, 0.8249)),
    ];
    if let Some(p) = cicp_primaries {
        tags.push(("cicp", cicp_type(p)));
    }
    tags.sort_by(|a, b| a.0.cmp(b.0));

    let mut header = vec![0u8; 128];
    header[4..8].copy_from_slice(b"lcms");
    // cicp 标签属于 ICC v4.4（ICC.1:2022）；普通 RGB profile 写 v4.3
    let version = if cicp_primaries.is_some() {
        0x04400000u32
    } else {
        0x04300000u32
    };
    header[8..12].copy_from_slice(&version.to_be_bytes());
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
    for (i, (name_bytes, data)) in tags.iter().enumerate() {
        tag_table[4 + i * 12..8 + i * 12].copy_from_slice(name_bytes.as_bytes());
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

/// PQ EOTF（SMPTE ST 2084），归一化到 [0,1]（1 = 10000 尼特）。用于生成 PQ ICC 的 `curv` 表。
fn pq_eotf_normalized(v: f64) -> f64 {
    let e = clamp(v, 0.0, 1.0);
    let m1 = 0.1593017578125;
    let m2 = 78.84375;
    let c1 = 0.8359375;
    let c2 = 18.8515625;
    let c3 = 18.6875;
    let e1 = e.powf(1.0 / m2);
    let num = (e1 - c1).max(0.0);
    let den = c2 - c3 * e1;
    (num / den).powf(1.0 / m1).clamp(0.0, 1.0)
}

/// 把 PQ 曲线采样成 ICC `curv` 标签（`count > 1` 即查表，1024 点足够）。
fn pq_curv(count: usize) -> Vec<u8> {
    let n = count.max(2);
    let mut buf = vec![0u8; 12 + n * 2];
    buf[0..4].copy_from_slice(b"curv");
    buf[8..12].copy_from_slice(&(n as u32).to_be_bytes());
    for i in 0..n {
        let x = i as f64 / (n - 1) as f64;
        let y = pq_eotf_normalized(x);
        let v = (y * 65535.0).round().clamp(0.0, 65535.0) as u16;
        buf[12 + i * 2..14 + i * 2].copy_from_slice(&v.to_be_bytes());
    }
    buf
}

/// 生成 ICC 路径（png / jpg_icc）的 **PQ** 输出色域 profile。
///
/// `assets/2020_profile.icc` 是现成的 Rec.2020 + PQ profile（含 cicp/mAB），直接沿用；
/// P3 / sRGB 没有现成的 PQ 资产（`display_p3_primary.icc` 是 sRGB TRC，不能用于 PQ 像素），
/// 这里按主色 + PQ `curv` + `cicp` 程序化生成。
pub fn build_pq_icc_for_gamut(gamut: &str) -> Option<Vec<u8>> {
    match gamut {
        "p3" | "display-p3" | "displayp3" => Some(build_rgb_icc(
            "Display P3 with PQ Transfer",
            [0.515102, 0.241186, -0.001126],
            [0.291979, 0.692219, 0.041882],
            [0.157101, 0.066593, 0.784072],
            pq_curv(1024),
            Some(12),
        )),
        "srgb" | "709" => Some(build_rgb_icc(
            "sRGB with PQ Transfer",
            [0.436041, 0.222485, 0.013920],
            [0.385113, 0.716879, 0.097109],
            [0.143051, 0.060608, 0.713913],
            pq_curv(1024),
            Some(1),
        )),
        _ => None,
    }
}

/// 已知主色（ICC rXYZ 标签 s15Fixed16 值；与 colorspace.rs 常量同源）。
/// Display-P3 主图 ICC 直接复用 Google 官方 P3 profile（`uhdr_primary_icc()`），无需自建常量。
const ADOBE_R_ICC: [f64; 3] = [0.609740, 0.205940, 0.149190];
const ADOBE_G_ICC: [f64; 3] = [0.311110, 0.625710, 0.063250];
const ADOBE_B_ICC: [f64; 3] = [0.125710, 0.070240, 0.991050];
const REC2020_R_ICC: [f64; 3] = [0.673462, 0.279038, -0.001938];
const REC2020_G_ICC: [f64; 3] = [0.165665, 0.675339, 0.029984];
const REC2020_B_ICC: [f64; 3] = [0.125107, 0.045624, 0.797165];
const DCIP3_R_ICC: [f64; 3] = [0.515120, 0.239993, -0.001161];
const DCIP3_G_ICC: [f64; 3] = [0.287340, 0.687020, 0.040835];
const DCIP3_B_ICC: [f64; 3] = [0.156957, 0.066161, 0.783875];
const PROPHOTO_R_ICC: [f64; 3] = [0.797766, 0.288041, 0.000000];
const PROPHOTO_G_ICC: [f64; 3] = [0.288041, 0.712137, 0.000014];
const PROPHOTO_B_ICC: [f64; 3] = [0.135126, 0.035061, 0.723524];

/// 按输入色彩空间生成主图 ICC：
/// - `embedded_icc`（原图自带 ICC，且为合法 RGB 显示 profile）→ 原汤化原食，直接沿用原 ICC 字节；
/// - 否则按检测空间程序化生成（Display-P3 用 Google 官方 P3 profile；sRGB 用程序化 sRGB；
///   Adobe RGB gamma 2.2 / DCI-P3 gamma 2.6 / ProPhoto gamma 1.8 / Rec.2020 gamma 2.4 近似）。
pub fn icc_for_input_space(space: InputColorSpace, embedded_icc: Option<&[u8]>) -> Vec<u8> {
    if let Some(icc) = embedded_icc {
        if icc.len() >= 132 && &icc[36..40] == b"acsp" && icc.len() >= 20 && &icc[16..20] == b"RGB "
        {
            return icc.to_vec();
        }
    }
    match space {
        InputColorSpace::Srgb => build_srgb_icc(),
        InputColorSpace::DisplayP3 => uhdr_primary_icc().to_vec(),
        InputColorSpace::AdobeRgb => build_rgb_icc(
            "Adobe RGB (1998)",
            ADOBE_R_ICC,
            ADOBE_G_ICC,
            ADOBE_B_ICC,
            para_type0(2.2),
            None,
        ),
        InputColorSpace::Rec2020 => build_rgb_icc(
            "Rec.2020 (BT.2020)",
            REC2020_R_ICC,
            REC2020_G_ICC,
            REC2020_B_ICC,
            para_type0(2.4),
            None,
        ),
        InputColorSpace::DciP3 => build_rgb_icc(
            "DCI-P3 (D65)",
            DCIP3_R_ICC,
            DCIP3_G_ICC,
            DCIP3_B_ICC,
            para_type0(2.6),
            None,
        ),
        InputColorSpace::ProPhoto => build_rgb_icc(
            "ProPhoto RGB (ROMM)",
            PROPHOTO_R_ICC,
            PROPHOTO_G_ICC,
            PROPHOTO_B_ICC,
            para_type0(1.8),
            None,
        ),
        InputColorSpace::Unknown => build_srgb_icc(),
    }
}

// ============================================================
//  总入口（← encode，行 954）
// ============================================================

/// 将图像编码为 Ultra HDR JPEG。
///
/// 主图策略（原汤化原食，2026 起为默认）：
/// - 主图像素**保持原始输入像素，不做色域转换**（不再默认 sRGB→Display-P3）；
/// - 主图 ICC 按输入色彩空间选：原图自带合法 ICC（`embedded_icc`）→ 直接沿用原 ICC；
///   否则按检测空间程序化生成（sRGB/P3/Adobe/2020/DCI-P3/ProPhoto 各自主色 + 传递函数）；
///   检测不到 → 按 sRGB 假设（sRGB ICC）。
/// - `settings.primary_srgb=true`（兼容旧语义）→ 强制 sRGB 主图 ICC（像素保持原样）。
///
/// ## 为什么这里**不做**色域拓展（gamut extension）
///
/// Ultra HDR 的「HDR」全部由增益图承载：增益图是**单通道** log2 增益，重建时对 R/G/B
/// 乘**同一个**系数（`hdr = sdr * 2^gain`），因此只改变亮度，色相/彩度/色度守恒；且规范
/// 规定 **HDR 图的色彩空间 = SDR 主图的色彩空间**（主图 ICC 即该声明，见 `colorspace.rs`
/// 顶部对规范的引用）。增益图元数据里也没有「目标色域」这种字段。
///
/// 所以在这一侧做色度拉伸（把 sRGB 撑到 P3/BT.2020）会同时违反两件事：
/// 1. 格式语义——解码端按主图 ICC 解释重建结果，拉伸后的颜色没有声明可依；
/// 2. 与 `settings.output_gamut` 无关——Ultra HDR 的「输出色域」由主图像素/ICC 决定
///    （原汤化原食），UI 也对该格式隐藏了「输出色域」选项。
///
/// 色域拓展只存在于 ICC 路径（png / jpg-icc，见 `convert::apply_hdr_rec2020_pq`）：
/// 那条链路会重新编码像素并注入「目标色域 + PQ」的 ICC，拉伸后的颜色有明确声明。
pub fn encode_ultra_hdr(
    primary_rgba: &[u8],
    width: u32,
    height: u32,
    settings: &Settings,
    detected: Option<&crate::colorspace::DetectedColorSpace>,
) -> Result<Vec<u8>> {
    let gain_map_scale = settings.gain_scale.max(1);
    // 原汤化原食：主图像素不转换（输入像素即主图像素）。
    // 注意：增益图/亮度统计按其原始编码（sRGB 近似线性化）计算——与 Kotlin 对 P3 输入行为一致。
    let main_rgba = primary_rgba.to_vec();
    let primary_icc = if settings.primary_srgb {
        // 兼容旧语义：主图强制 sRGB（Google 官方 sRGB ICC）
        uhdr_gainmap_icc().to_vec()
    } else {
        let space = detected
            .map(|d| d.space)
            .unwrap_or(InputColorSpace::Unknown);
        icc_for_input_space(space, detected.and_then(|d| d.embedded_icc.as_deref()))
    };

    // 1. 增益图（SDR 基准 = mainRgba 的线性亮度，即主图色彩空间；GPU 优先）。
    // GPU 路径与 CPU 同式（B 曲线 + 局部镜面高光），元数据也由 GPU 入口直接给出；
    // 输入按 settings.gain_scale 先 box 下采样到增益图分辨率（两边一致）。
    let (gm8, meta) =
        match crate::gpu::try_gpu_compute_gainmap(&main_rgba, width, height, settings) {
            Some(v) => v,
            None => compute_gain_map(&main_rgba, width as usize, height as usize, settings),
        };
    let gm_w = (width / gain_map_scale).max(1);
    let gm_h = (height / gain_map_scale).max(1);
    debug_assert_eq!(
        gm8.len() as u32,
        gm_w * gm_h,
        "compute_gain_map 应返回低分辨率 gm8（{}x{}），实际 {} 字节",
        gm_w,
        gm_h,
        gm8.len()
    );

    // 2. 增益图灰度基线 JPEG
    let quality = settings.quality.clamp(0.1, 1.0);
    let gm_jpeg = encode_jpeg_gray(&gm8, gm_w, gm_h, quality)?;

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
