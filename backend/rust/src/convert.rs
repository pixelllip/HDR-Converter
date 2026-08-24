//! ← HdrConverter.kt：像素变换核心 + 图像 I/O。
//!
//! 移植对照：
//! - `applyHdrTransformToRec2020Pq` (HdrConverter.kt:149) → `apply_hdr_rec2020_pq`
//!   —— `/convert` 的 png / jpg_icc 实际走这条管线：sRGB→线性（无自动伽马）→
//!     RGB 通道 × 曝光 → 伽马 → Rec.709→Rec.2020 → PQ 编码 + whiteNits/10000 缩放
//! - `applyHdrTransform` (HdrConverter.kt:52) → `apply_hdr_transform_legacy`
//!   —— 旧版（自动伽马 + sRGB 编码），当前 Kotlin /convert 已不再调用，保留作参考/对照
//! - `pqEncode` (HdrConverter.kt:217) → `pq_encode`（SMPTE ST 2084）
//! - `readImageAsRgba` (HdrConverter.kt:235) → `read_image_rgba`
//! - `encodeJpeg` (HdrConverter.kt:285) → `encode_jpeg`
//!
//! 数值约定：与 Kotlin 逐位一致 —— float64 全程运算、不做中间裁剪
//! （仅 pow 前 clamp ≥0、PQ 入参 clamp [0,1]、最终字节 round+clamp 0..255）。
//! `Math.round(x)`（Kotlin，向 +∞）与 `f64::round`（Rust，远离 0）对 x≥0 结果相同，
//! 本管线所有取值均非负，等价。

use std::path::Path;

use anyhow::{Context, Result};
use rayon::prelude::*;

use crate::models::Settings;

/// 解码后的 RGBA 图像（8-bit/通道），对应 Kotlin `ImageData`。
#[derive(Debug, Clone)]
pub struct ImageData {
    pub pixels: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// ← `HdrConverter.srgbToLinear`（HdrConverter.kt:25）
fn srgb_to_linear(value: f64) -> f64 {
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

/// ← `HdrConverter.linearToSrgb`（HdrConverter.kt:30）—— 仅 legacy 变换使用
fn linear_to_srgb(value: f64) -> f64 {
    if value <= 0.0031308 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    }
}

/// ← `HdrConverter.pqEncode`（HdrConverter.kt:217）：线性亮度 L∈[0,1]（相对 10000 尼特）→ PQ 码 0..1
fn pq_encode(l: f64) -> f64 {
    let ll = l.clamp(0.0, 1.0);
    let m1 = 0.159_301_757_812_5;
    let m2 = 78.84375;
    let c1 = 0.8359375;
    let c2 = 18.8515625;
    let c3 = 18.6875;
    let lm1 = ll.powf(m1);
    ((c1 + c2 * lm1) / (1.0 + c3 * lm1)).powf(m2)
}

/// 读取图像为 RGBA（← `HdrConverter.readImageAsRgba`）。
pub fn read_image_rgba(path: &Path) -> Result<ImageData> {
    let img =
        image::open(path).with_context(|| format!("无法读取图像: {}", path.display()))?;
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();
    Ok(ImageData {
        pixels: rgba.into_raw(),
        width,
        height,
    })
}

/// 图片 HDR 变换 → Rec.2020/PQ 编码（← `HdrConverter.applyHdrTransformToRec2020Pq`，HdrConverter.kt:149）。
///
/// `/convert` 的 png / jpg_icc 输出即此管线（再分别注入 2020 / P3 ICC）。
/// 变换语义：无自动伽马（与视频预览一致，避免偏暗/发白）——
/// 曝光 = 峰值/白点 = 2^EV（微调明暗已移除）。
///
/// 参数：白点取 `settings.white_nits`（Kotlin 侧 `whiteNits ?: 203.0`，本 CLI 默认即 203）。
pub fn apply_hdr_rec2020_pq(img: &ImageData, settings: &Settings) -> Result<ImageData> {
    let total_pixels = img.width as usize * img.height as usize;
    let exposure = settings.peak_nits / settings.white_nits; // Kotlin: (peakNits ?: 1000) / whiteNits
    let r_adj = settings.rgb.red;
    let g_adj = settings.rgb.green;
    let b_adj = settings.rgb.blue;
    let gamma = settings.gamma;
    let scale = settings.white_nits / 10_000.0; // ← Kotlin: scale = whiteNits / 10000.0

    // Pass 1: sRGB → 线性（不做自动伽马；并行，结果与顺序逐位一致）
    let linear: Vec<f64> = (0..total_pixels)
        .into_par_iter()
        .flat_map_iter(|i| {
            let base = i * 4;
            [
                srgb_to_linear(img.pixels[base] as f64 / 255.0),
                srgb_to_linear(img.pixels[base + 1] as f64 / 255.0),
                srgb_to_linear(img.pixels[base + 2] as f64 / 255.0),
            ]
            .into_iter()
        })
        .collect();

    // Pass 3: RGB 通道 × 曝光 → 伽马 → Rec.709→Rec.2020 → PQ（并行）
    let out: Vec<u8> = (0..total_pixels)
        .into_par_iter()
        .flat_map_iter(|i| {
            let o = i * 3;
            // Kotlin: var r = linear[offset] * rAdj * exposure（左结合乘法，顺序一致）
            let r = linear[o] * r_adj * exposure;
            let g = linear[o + 1] * g_adj * exposure;
            let b = linear[o + 2] * b_adj * exposure;
            // Kotlin: Math.pow(Math.max(r, 0.0), gamma)
            let r = r.max(0.0).powf(gamma);
            let g = g.max(0.0).powf(gamma);
            let b = b.max(0.0).powf(gamma);
            // Rec.709 → Rec.2020（与视频 zscale pin=bt709 → p=bt2020 一致，常量逐位照抄）
            let r2020 = 0.6274038959 * r + 0.3292830384 * g + 0.0433130642 * b;
            let g2020 = 0.0690972894 * r + 0.9195403951 * g + 0.0113623156 * b;
            let b2020 = 0.0163914389 * r + 0.0880133078 * g + 0.8955952528 * b;
            // Kotlin: Math.round(pqEncode(x) * 255).toInt().coerceIn(0,255)
            [
                (pq_encode(r2020 * scale) * 255.0).round().clamp(0.0, 255.0) as u8,
                (pq_encode(g2020 * scale) * 255.0).round().clamp(0.0, 255.0) as u8,
                (pq_encode(b2020 * scale) * 255.0).round().clamp(0.0, 255.0) as u8,
                255,
            ]
            .into_iter()
        })
        .collect();

    Ok(ImageData {
        pixels: out,
        width: img.width,
        height: img.height,
    })
}

/// 旧版 HDR 变换（← `HdrConverter.applyHdrTransform`，HdrConverter.kt:52，**当前 /convert 已不使用**）。
///
/// 与 Rec.2020/PQ 管线的差异：含「自动伽马」（按平均亮度自适应，0.3~3.0 截断）、
/// 末尾 sRGB 编码。保留用于与 Kotlin 对照/历史行为验证。
///
/// @param total_exposure 总曝光。Kotlin 侧 = `hdrIntensity * fineTuneBrightness`
///   （旧 UI 语义；新 UI 已移除微调明暗，改用峰值/白点，见 `apply_hdr_rec2020_pq`）。
pub fn apply_hdr_transform_legacy(
    img: &ImageData,
    settings: &Settings,
    total_exposure: f64,
) -> Result<ImageData> {
    let total_pixels = img.width as usize * img.height as usize;

    // Pass 1: sRGB → 线性 + 平均亮度
    let mut linear = vec![0.0f64; total_pixels * 3];
    let mut sum = 0.0f64;
    for i in 0..total_pixels {
        let base = i * 4;
        let r = srgb_to_linear(img.pixels[base] as f64 / 255.0);
        let g = srgb_to_linear(img.pixels[base + 1] as f64 / 255.0);
        let b = srgb_to_linear(img.pixels[base + 2] as f64 / 255.0);
        let o = i * 3;
        linear[o] = r;
        linear[o + 1] = g;
        linear[o + 2] = b;
        sum += 0.2126 * r + 0.7152 * g + 0.0722 * b;
    }

    // Pass 2: 自动伽马（基于平均亮度自适应）
    let mean = sum / total_pixels as f64;
    if mean > 0.001 && mean < 0.999 {
        let auto_gamma = 0.5f64.ln() / mean.ln();
        let clamped = auto_gamma.clamp(0.3, 3.0);
        for v in linear.iter_mut() {
            *v = v.max(0.0).powf(clamped);
        }
    }

    // Pass 3: RGB 通道调整 + 曝光 + 伽马 + sRGB 编码
    let r_adj = settings.rgb.red;
    let g_adj = settings.rgb.green;
    let b_adj = settings.rgb.blue;
    let gamma = settings.gamma;

    let mut out = vec![0u8; total_pixels * 4];
    for i in 0..total_pixels {
        let o = i * 3;
        let mut r = linear[o] * r_adj;
        let mut g = linear[o + 1] * g_adj;
        let mut b = linear[o + 2] * b_adj;
        r *= total_exposure;
        g *= total_exposure;
        b *= total_exposure;
        r = r.max(0.0).powf(gamma);
        g = g.max(0.0).powf(gamma);
        b = b.max(0.0).powf(gamma);
        let sr = linear_to_srgb(r).clamp(0.0, 1.0);
        let sg = linear_to_srgb(g).clamp(0.0, 1.0);
        let sb = linear_to_srgb(b).clamp(0.0, 1.0);
        let o = i * 4;
        out[o] = (sr * 255.0).round().clamp(0.0, 255.0) as u8;
        out[o + 1] = (sg * 255.0).round().clamp(0.0, 255.0) as u8;
        out[o + 2] = (sb * 255.0).round().clamp(0.0, 255.0) as u8;
        out[o + 3] = 255;
    }

    Ok(ImageData {
        pixels: out,
        width: img.width,
        height: img.height,
    })
}

/// 编码为 PNG 字节（iCCP 注入前使用）。
pub fn encode_png_bytes(img: &ImageData) -> Result<Vec<u8>> {
    use image::ImageEncoder;
    let mut buf = Vec::new();
    let enc = image::codecs::png::PngEncoder::new(&mut buf);
    enc.write_image(
        &img.pixels,
        img.width,
        img.height,
        image::ExtendedColorType::Rgba8,
    )
    .context("PNG 编码失败")?;
    Ok(buf)
}

/// 编码为 PNG。
pub fn encode_png(img: &ImageData, out: &Path) -> Result<()> {
    let bytes = encode_png_bytes(img)?;
    std::fs::write(out, bytes).with_context(|| format!("写入 PNG 失败: {}", out.display()))?;
    Ok(())
}

/// 编码为 JPEG 字节（APP2 ICC 注入前使用；quality 0..1）。
pub fn encode_jpeg_bytes(img: &ImageData, quality: f64) -> Result<Vec<u8>> {
    let q = (quality.clamp(0.0, 1.0) * 100.0).round().clamp(1.0, 100.0) as u8;
    let mut buf = Vec::new();
    let mut rgb = Vec::with_capacity(img.width as usize * img.height as usize * 3);
    for px in img.pixels.chunks_exact(4) {
        rgb.extend_from_slice(&px[..3]);
    }
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, q);
    enc.encode(&rgb, img.width, img.height, image::ExtendedColorType::Rgb8)
        .context("JPEG 编码失败")?;
    Ok(buf)
}

/// 编码为 JPEG（quality 0..1，对应 Kotlin `HdrConverter.encodeJpeg` 的 quality 参数）。
/// JPEG 无 alpha 通道：内部先把 RGBA 转 RGB 再编码。
pub fn encode_jpeg(img: &ImageData, out: &Path, quality: f64) -> Result<()> {
    let bytes = encode_jpeg_bytes(img, quality)?;
    std::fs::write(out, bytes).with_context(|| format!("写入 JPEG 失败: {}", out.display()))?;
    Ok(())
}