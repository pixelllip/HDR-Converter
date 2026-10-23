//! ← HdrConverter.kt：像素变换核心 + 图像 I/O。
//!
//! 移植对照：
//! - `applyHdrTransformToRec2020Pq` (HdrConverter.kt:149) → `apply_hdr_rec2020_pq`
//!   —— `/convert` 的 png / jpg_icc 实际走这条管线：sRGB→线性（无自动伽马）→
//!     明度 B 曲线（中调抬升 + 白点滚降）→ BT.709→输出色域 → PQ 编码 + whiteNits/10000 缩放
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

/// ← `HdrConverter.srgbToLinear`（HdrConverter.kt:25）——默认输入传函（sRGB）；
/// 输入解读参数化在 crate::colorspace::InputCodec（P1 内容区参数）。
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
    let img = image::open(path).with_context(|| format!("无法读取图像: {}", path.display()))?;
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();
    Ok(ImageData {
        pixels: rgba.into_raw(),
        width,
        height,
    })
}

// ============================================================
//  输出色域拓展（gamut extension）——仅 ICC 路径（png / jpg_icc）
// ============================================================
//
//  「输出色域」本身只是 colorimetric 转换（BT.709→P3/2020 矩阵），色度守恒、观感不变；
//  要让 sRGB 内容真正用上 P3 / BT.2020 更大的色域体积，必须显式做**色度拉伸**。这里的
//  做法是标准的 gamut extension：在 Oklab 里保持色相与明度，把彩度按
//  「目标色域边界 / sRGB 边界」的比值放大——sRGB 的色域边界正好落到目标色域边界上，
//  从而「用满」目标色域而不越界（`settings.gamut_expand` 线性插值强度，0 = 关闭）。
//
//  ⚠️ **Ultra HDR 路径刻意不做这一步**（见 `ultra_hdr.rs::encode_ultra_hdr` 的说明）：
//  增益图是单通道 log2 增益、对 R/G/B 乘同一系数，且规范规定 HDR 图色彩空间 = SDR 主图
//  色彩空间——那里做色度拉伸既违反格式语义，也会让解码端按主图 ICC 解释出错误颜色。

/// BT.709 → Display-P3（D65 线性矩阵，与 Kotlin 版常量逐位一致）。
const M_709_TO_P3: [[f64; 3]; 3] = [
    [0.822462, 0.177538, 0.0],
    [0.033195, 0.966805, 0.0],
    [0.017082, 0.072397, 0.910521],
];

/// BT.709 → BT.2020（D65 线性矩阵）。
const M_709_TO_2020: [[f64; 3]; 3] = [
    [0.6274038959, 0.3292830384, 0.0433130642],
    [0.0690972894, 0.9195403951, 0.0113623156],
    [0.0163914389, 0.0880133078, 0.8955952528],
];

fn mat3_mul(m: &[[f64; 3]; 3], v: (f64, f64, f64)) -> (f64, f64, f64) {
    (
        m[0][0] * v.0 + m[0][1] * v.1 + m[0][2] * v.2,
        m[1][0] * v.0 + m[1][1] * v.1 + m[1][2] * v.2,
        m[2][0] * v.0 + m[2][1] * v.1 + m[2][2] * v.2,
    )
}

/// 输出色域矩阵；`None` = BT.709/sRGB（恒等，无需转换）。
fn out_gamut_matrix(gamut: &str) -> Option<[[f64; 3]; 3]> {
    match gamut {
        "p3" | "display-p3" | "displayp3" => Some(M_709_TO_P3),
        "srgb" | "709" => None,
        _ => Some(M_709_TO_2020),
    }
}

/// 线性 sRGB → Oklab（Björn Ottosson 2020）。L∈[0,1]、a/b 为彩度分量。
fn oklab_from_linear_srgb(r: f64, g: f64, b: f64) -> (f64, f64, f64) {
    let l = 0.412_221_470_8 * r + 0.536_332_536_3 * g + 0.051_445_992_9 * b;
    let m = 0.211_903_498_2 * r + 0.680_699_545_1 * g + 0.107_396_956_6 * b;
    let s = 0.088_302_461_9 * r + 0.281_718_837_6 * g + 0.629_978_700_5 * b;
    let (l_, m_, s_) = (l.cbrt(), m.cbrt(), s.cbrt());
    (
        0.210_454_255_3 * l_ + 0.793_617_785_0 * m_ - 0.004_072_046_8 * s_,
        1.977_998_495_1 * l_ - 2.428_592_205_0 * m_ + 0.450_593_709_9 * s_,
        0.025_904_037_1 * l_ + 0.782_771_766_2 * m_ - 0.808_675_766_0 * s_,
    )
}

/// Oklab → 线性 sRGB。
fn oklab_to_linear_srgb(l: f64, a: f64, b: f64) -> (f64, f64, f64) {
    let l_ = l + 0.396_337_777_4 * a + 0.215_803_757_3 * b;
    let m_ = l - 0.105_561_345_8 * a - 0.063_854_172_8 * b;
    let s_ = l - 0.089_484_177_5 * a - 1.291_485_548_0 * b;
    let (l3, m3, s3) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);
    (
        4.076_741_662_1 * l3 - 3.307_711_591_3 * m3 + 0.230_969_929_2 * s3,
        -1.268_438_004_6 * l3 + 2.609_757_401_1 * m3 - 0.341_319_396_5 * s3,
        -0.004_196_086_3 * l3 - 0.703_418_614_7 * m3 + 1.707_614_701_0 * s3,
    )
}

/// Oklab 色 (L, C, h) 是否仍落在目标色域立方体 [0,1]³ 内（`None` = BT.709 立方体）。
fn gamut_fits(target: Option<&[[f64; 3]; 3]>, l: f64, c: f64, h: f64) -> bool {
    let rgb = oklab_to_linear_srgb(l, c * h.cos(), c * h.sin());
    let (r, g, b) = match target {
        Some(m) => mat3_mul(m, rgb),
        None => rgb,
    };
    const E: f64 = 1e-9;
    r >= -E && g >= -E && b >= -E && r <= 1.0 + E && g <= 1.0 + E && b <= 1.0 + E
}

/// 给定 (L, h)，二分求目标色域内仍合法的最大 Oklab 彩度。
fn max_chroma(target: Option<&[[f64; 3]; 3]>, l: f64, h: f64) -> f64 {
    if !gamut_fits(target, l, 0.0, h) {
        return 0.0; // L 已超出色域（理论上不会：L 取自归一化颜色）
    }
    let (mut lo, mut hi) = (0.0f64, 0.6f64);
    for _ in 0..24 {
        let mid = 0.5 * (lo + hi);
        if gamut_fits(target, l, mid, h) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    lo
}

/// (L, hue) → 彩度放大比 `C_max(目标) / C_max(sRGB)` 的二维查表。
///
/// 逐像素二分不可接受（4K 图 ≈ 8M 次×24 轮），故按 33×65 网格预计算 + 双线性插值；
/// 表只依赖输出色域，与图像内容无关。
struct GamutExpandTable {
    n_l: usize,
    n_h: usize,
    ratio: Vec<f64>,
}

impl GamutExpandTable {
    fn build(target: &[[f64; 3]; 3]) -> Self {
        let (n_l, n_h) = (33usize, 65usize);
        let mut ratio = vec![1.0f64; n_l * n_h];
        for i in 0..n_l {
            let l = i as f64 / (n_l - 1) as f64;
            for j in 0..n_h {
                let h = (j as f64 / n_h as f64) * std::f64::consts::TAU;
                let cs = max_chroma(None, l, h);
                let ct = max_chroma(Some(target), l, h);
                // sRGB 边界在 L→1 时收缩到 0：比值无意义，保持 1.0（近白处彩度本就极小）
                if cs > 1e-4 {
                    ratio[i * n_h + j] = (ct / cs).clamp(1.0, 3.0);
                }
            }
        }
        Self { n_l, n_h, ratio }
    }

    /// 双线性查表（h 环形闭合）。
    fn lookup(&self, l: f64, h: f64) -> f64 {
        let x = l.clamp(0.0, 1.0) * (self.n_l - 1) as f64;
        let i0 = (x.floor() as usize).min(self.n_l - 1);
        let i1 = (i0 + 1).min(self.n_l - 1);
        let fx = x - i0 as f64;

        let t = (h / std::f64::consts::TAU).rem_euclid(1.0) * self.n_h as f64;
        let j0 = (t.floor() as usize) % self.n_h;
        let j1 = (j0 + 1) % self.n_h;
        let fy = t - t.floor();

        let at = |i: usize, j: usize| self.ratio[i * self.n_h + j];
        let a = at(i0, j0) * (1.0 - fy) + at(i0, j1) * fy;
        let b = at(i1, j0) * (1.0 - fy) + at(i1, j1) * fy;
        a * (1.0 - fx) + b * fx
    }
}

/// 在 BT.709 线性光上做色度拉伸：色相/明度不变，彩度按 `1 + strength*(k-1)` 放大。
///
/// 归一化到「最大分量 = 1」再做 Oklab，保证同一色相在任意亮度下放大比例一致
/// （色域是锥体，色度与绝对亮度无关），最后按原最大分量缩放回来。
fn expand_chroma_709(
    r: f64,
    g: f64,
    b: f64,
    table: &GamutExpandTable,
    strength: f64,
) -> (f64, f64, f64) {
    let mx = r.max(g).max(b);
    if !(mx > 1e-9) || strength <= 0.0 {
        return (r, g, b);
    }
    let (nr, ng, nb) = (r / mx, g / mx, b / mx);
    let (l, a, bb) = oklab_from_linear_srgb(nr, ng, nb);
    let c = (a * a + bb * bb).sqrt();
    if c <= 1e-6 {
        return (r, g, b); // 中性色：无色度可拉伸
    }
    let k = 1.0 + strength * (table.lookup(l, bb.atan2(a)) - 1.0);
    let (er, eg, eb) = oklab_to_linear_srgb(l, a * k, bb * k);
    (er * mx, eg * mx, eb * mx)
}

/// 图片 HDR 变换 → 输出色域/PQ 编码（← `HdrConverter.applyHdrTransformToRec2020Pq`，HdrConverter.kt:149）。
///
/// `/convert` 的 png / jpg_icc 输出即此管线（再按 `output_gamut` 注入对应 ICC）。
/// 变换语义：无自动伽马（与视频预览一致，避免偏暗/发白）——
/// 明度 B 曲线（中调抬升 + 白点按 `settings.rolloff` 滚降），与 Ultra HDR 增益图共用同一函数。
///
/// 输出色域：先按 `settings.gamut_expand` 做色度拉伸（0 = 纯 colorimetric 转换，
/// 1 = 用满目标色域；见上方模块注释），再乘 BT.709→目标色域矩阵。
///
/// 参数：白点取 `settings.white_nits`（Kotlin 侧 `whiteNits ?: 203.0`，本 CLI 默认即 203）。
pub fn apply_hdr_rec2020_pq(img: &ImageData, settings: &Settings) -> Result<ImageData> {
    let total_pixels = img.width as usize * img.height as usize;
    let exposure = settings.peak_nits / settings.white_nits; // 峰值/白点
    let rolloff = settings.rolloff.clamp(0.0, 1.0);
    let image_gamma = settings.image_gamma.clamp(0.1, 2.0);
    let scale = settings.white_nits / 10_000.0; // ← Kotlin: scale = whiteNits / 10000.0
    let out_gamut = settings.output_gamut.as_deref().unwrap_or("2020");

    // 色域拓展：sRGB 输出（恒等矩阵）无需拉伸，直接跳过（也省掉建表开销）
    let out_matrix = out_gamut_matrix(out_gamut);
    let expand_strength = settings.gamut_expand.clamp(0.0, 1.0);
    let expand_table = match (out_matrix.as_ref(), expand_strength > 0.0) {
        (Some(m), true) => Some(GamutExpandTable::build(m)),
        _ => None,
    };

    // Pass 1: 输入解读（传递函数 EOTF + 输入色域 → BT.709）→ 线性（并行，结果与顺序逐位一致）
    let codec = crate::colorspace::InputCodec::from_settings(settings);
    let linear: Vec<f64> = (0..total_pixels)
        .into_par_iter()
        .flat_map_iter(|i| {
            let base = i * 4;
            let (r, g, b) = codec.to_linear(
                img.pixels[base] as f64 / 255.0,
                img.pixels[base + 1] as f64 / 255.0,
                img.pixels[base + 2] as f64 / 255.0,
            );
            [r, g, b].into_iter()
        })
        .collect();

    // Pass 2: 画面最亮点 → 与 Ultra HDR 相同的峰值锚定（m = 曝光 / y_max；低动态范围图直接用曝光）
    let y_max = (0..total_pixels)
        .into_par_iter()
        .map(|i| {
            let o = i * 3;
            0.2126 * linear[o] + 0.7152 * linear[o + 1] + 0.0722 * linear[o + 2]
        })
        .reduce(|| 0.0f64, f64::max);
    let tone_m = crate::ultra_hdr::tone_exposure(exposure, y_max);

    // Pass 3: 明度增益（ICC 与 Ultra HDR 共用 tone 曲线）→ 输出色域 → PQ（并行）
    let out: Vec<u8> = (0..total_pixels)
        .into_par_iter()
        .flat_map_iter(|i| {
            let o = i * 3;
            let r = linear[o];
            let g = linear[o + 1];
            let b = linear[o + 2];
            // 按明度加增益：与 Ultra HDR 的全局 B 曲线同式（伽马 + 中调抬升 + 白点滚降）。
            // `.min(tone_m)` 与增益图侧 `g.min(m_eff)` 对齐：γ<1 时曲线会把中调顶过峰值，
            // 两侧都必须在峰值处钳住，否则 ICC 与 Ultra HDR 会分叉（实测 γ=0.8 差 ~1/4）。
            let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
            let gain = crate::ultra_hdr::tone_gain(y, tone_m, rolloff, image_gamma).min(tone_m);
            let mut r = (r * gain).max(0.0);
            let mut g = (g * gain).max(0.0);
            let mut b = (b * gain).max(0.0);
            // 色域拓展（gamut extension）：沿 Oklab 色相拉伸彩度，用满目标色域
            if let Some(t) = expand_table.as_ref() {
                let (er, eg, eb) = expand_chroma_709(r, g, b, t, expand_strength);
                r = er;
                g = eg;
                b = eb;
            }
            // BT.709 → 输出色域（扩张后的彩度可能落在 BT.709 负值区，由矩阵转回目标色域的正值域）
            let (ro, go, bo) = match out_matrix.as_ref() {
                Some(m) => mat3_mul(m, (r, g, b)),
                None => (r, g, b),
            };
            [
                (pq_encode(ro * scale) * 255.0).round().clamp(0.0, 255.0) as u8,
                (pq_encode(go * scale) * 255.0).round().clamp(0.0, 255.0) as u8,
                (pq_encode(bo * scale) * 255.0).round().clamp(0.0, 255.0) as u8,
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

// ============================================================
//  EXIF Orientation + 预览读取（← HdrConverter.exifOrientation/applyOrientation/readImageForPreview）
// ============================================================

/// 读取 JPEG/EXIF Orientation（1..8）；非 JPEG / 无 EXIF 一律返回 1（← parseJpegExifOrientation + parseTiffOrientation）。
pub fn exif_orientation(path: &Path) -> u8 {
    match std::fs::metadata(path) {
        Ok(m) if !(m.len() > 4 && m.len() <= 64 * 1024 * 1024) => return 1,
        Err(_) => return 1,
        _ => {}
    }
    let data = match std::fs::read(path) {
        Ok(d) => d,
        Err(_) => return 1,
    };
    if data.len() < 4 || data[0] != 0xFF || data[1] != 0xD8 || data[2] != 0xFF {
        return 1; // 非 JPEG
    }
    let mut i = 2usize;
    while i + 3 < data.len() {
        if data[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = data[i + 1];
        if marker == 0xFF || marker == 0xD8 {
            i += 2;
            continue;
        }
        if marker == 0xD9 || marker == 0xDA {
            return 1; // EOI / SOS：之后不再有 APP1 EXIF
        }
        if marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            i += 2;
            continue;
        }
        if i + 4 > data.len() {
            return 1;
        }
        let len = ((data[i + 2] as usize) << 8) | data[i + 3] as usize;
        if len < 2 || i + 2 + len > data.len() {
            return 1;
        }
        if marker == 0xE1 && len >= 10 && &data[i + 4..i + 10] == b"Exif\0\0" {
            return parse_tiff_orientation(&data, i + 10);
        }
        i += 2 + len;
    }
    1
}

/// 解析 TIFF 头 + IFD0，找 Orientation (tag 0x0112, SHORT)（← Kotlin parseTiffOrientation）。
fn parse_tiff_orientation(data: &[u8], start: usize) -> u8 {
    if start + 8 > data.len() {
        return 1;
    }
    let le = data[start] == b'I' && data[start + 1] == b'I';
    let be = data[start] == b'M' && data[start + 1] == b'M';
    if !le && !be {
        return 1;
    }
    let u16v = |o: usize| -> usize {
        if le {
            data[o] as usize | ((data[o + 1] as usize) << 8)
        } else {
            ((data[o] as usize) << 8) | data[o + 1] as usize
        }
    };
    let u32v = |o: usize| -> usize {
        if le {
            data[o] as usize
                | ((data[o + 1] as usize) << 8)
                | ((data[o + 2] as usize) << 16)
                | ((data[o + 3] as usize) << 24)
        } else {
            ((data[o] as usize) << 24)
                | ((data[o + 1] as usize) << 16)
                | ((data[o + 2] as usize) << 8)
                | data[o + 3] as usize
        }
    };
    if u16v(start + 2) != 42 {
        return 1;
    }
    let ifd_start = start + u32v(start + 4);
    if ifd_start + 2 > data.len() {
        return 1;
    }
    let n = u16v(ifd_start);
    if ifd_start as u64 + 2 + n as u64 * 12 > data.len() as u64 {
        return 1;
    }
    for j in 0..n {
        let e = ifd_start + 2 + j * 12;
        if u16v(e) == 0x0112 {
            let v = u16v(e + 8);
            return if (1..=8).contains(&v) { v as u8 } else { 1 };
        }
    }
    1
}

/// 根据 EXIF Orientation 把存储像素转成「正向显示像素」（← Kotlin applyOrientation）。
///
/// 几何映射（与 Pillow/TwelveMonkeys 一致，5/6/7/8 交换宽高）：
/// 1 原图 · 2 水平翻转 · 3 旋转180 · 4 垂直翻转 · 5 主对角线镜像(转置) ·
/// 6 旋转90°CW · 7 副对角线镜像(横向翻转) · 8 旋转90°CCW。
fn apply_orientation(img: image::DynamicImage, orientation: u8) -> image::DynamicImage {
    match orientation {
        2 => image::imageops::flip_horizontal(&img).into(),
        3 => img.rotate180(),
        4 => image::imageops::flip_vertical(&img).into(),
        // 5 = transpose = rotate90CCW 后再垂直翻转（推导：(x,y)->(y,x)）
        5 => image::imageops::flip_vertical(&img.rotate90()).into(),
        6 => img.rotate270(), // 90°CW
        // 7 = transverse = rotate90CCW 后再水平翻转
        7 => image::imageops::flip_horizontal(&img.rotate90()).into(),
        8 => img.rotate90(),
        _ => img,
    }
}

/// 读取图片并缩放到预览尺寸（默认 50%），先按 EXIF orientation 转正再缩放
/// （← `HdrConverter.readImageForPreview`，HdrConverter.kt:302）。
pub fn read_image_for_preview(path: &Path, scale_ratio: f64) -> Result<ImageData> {
    let img = image::open(path).with_context(|| format!("无法读取图像: {}", path.display()))?;
    let img = apply_orientation(img, exif_orientation(path));
    let w = ((img.width() as f64) * scale_ratio) as u32; // Kotlin: .toInt() 截断
    let h = ((img.height() as f64) * scale_ratio) as u32;
    let w = w.max(1);
    let h = h.max(1);
    let scaled = if w == img.width() && h == img.height() {
        img
    } else {
        image::imageops::resize(&img, w, h, image::imageops::FilterType::Triangle).into()
    };
    let rgba = scaled.to_rgba8();
    let (width, height) = rgba.dimensions();
    Ok(ImageData {
        pixels: rgba.into_raw(),
        width,
        height,
    })
}
