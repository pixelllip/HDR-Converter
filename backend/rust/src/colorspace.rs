//! ← ColorSpaceDetector.kt：输入图像色彩空间探测。
//!
//! 依据 Ultra HDR 规范："SDR 图像的色彩配置定义了 HDR 图像的色彩空间"，
//! 不假设输入是 sRGB，先检测原图实际色彩空间（ICC / EXIF / JFIF / PNG 标记）。
//!
//! 检测顺序（与 Kotlin 一致）：ICC(APP2/iCCP) > EXIF ColorSpace > JFIF/PNG 标记 > UNKNOWN(默认 sRGB 假设)。
//! 移植对照：ColorSpaceDetector.kt —— detect (行 215) / detectJpeg (106) / detectPng (192)
//! / iccToColorSpace (76) / parseExifColorSpace (151) / matchPrimaries (55)。

use std::fmt;
use std::path::Path;

/// 输入图像色彩空间（← ColorSpaceDetector.kt `InputColorSpace`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputColorSpace {
    Srgb,
    DisplayP3,
    AdobeRgb,
    Unknown,
}

impl fmt::Display for InputColorSpace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 与 Kotlin displayName 一致
        let s = match self {
            Self::Srgb => "sRGB (IEC 61966-2-1)",
            Self::DisplayP3 => "Display-P3 (D65)",
            Self::AdobeRgb => "Adobe RGB (1998)",
            Self::Unknown => "未声明（按 sRGB 假设）",
        };
        f.write_str(s)
    }
}

// 已知基色（线性 XYZ，ICC s15Fixed16 归一化，rXYZ 标签值；← Kotlin 常量逐位照抄）
const SRGB_R: [f64; 3] = [0.436041, 0.222485, 0.013920];
const P3_R: [f64; 3] = [0.515102, 0.241186, -0.001126];
const ADOBE_R: [f64; 3] = [0.609740, 0.205940, 0.149190];
const SRGB_G: [f64; 3] = [0.385113, 0.716879, 0.097109];
const P3_G: [f64; 3] = [0.291979, 0.692219, 0.041882];
const ADOBE_G: [f64; 3] = [0.311110, 0.625710, 0.063250];
const SRGB_B: [f64; 3] = [0.143051, 0.060608, 0.713913];
const P3_B: [f64; 3] = [0.157101, 0.066593, 0.784072];
const ADOBE_B: [f64; 3] = [0.125710, 0.070240, 0.991050];

/// 读取文件头部若干字节用于元数据解析（色彩段都在头部；← readHead，max=256KB）。
fn read_head(path: &Path) -> Vec<u8> {
    match std::fs::File::open(path) {
        Ok(mut f) => {
            let mut buf = vec![0u8; 256 * 1024];
            use std::io::Read;
            let n = f.read(&mut buf).unwrap_or(0);
            buf.truncate(n);
            buf
        }
        Err(_) => Vec::new(),
    }
}

fn u16(b: &[u8], o: usize) -> usize {
    ((b[o] as usize) << 8) | (b[o + 1] as usize)
}

fn u32(b: &[u8], o: usize) -> u32 {
    ((b[o] as u32) << 24)
        | ((b[o + 1] as u32) << 16)
        | ((b[o + 2] as u32) << 8)
        | (b[o + 3] as u32)
}

/// s15Fixed16 → f64（有符号；← Kotlin `u32().toInt() / 65536.0`）。
fn s15fixed(b: &[u8], o: usize) -> f64 {
    (u32(b, o) as i32) as f64 / 65536.0
}

/// 基色匹配：比较三个主色（忽略微小差异），返回最接近的已知空间（← matchPrimaries，阈值 0.02）。
fn match_primaries(r: [f64; 3], g: [f64; 3], b: [f64; 3]) -> Option<InputColorSpace> {
    fn dist(a: [f64; 3], reference: [f64; 3]) -> f64 {
        let mut d = 0.0;
        for i in 0..3 {
            d += (a[i] - reference[i]) * (a[i] - reference[i]);
        }
        d.sqrt()
    }
    let sets: [([f64; 3], [f64; 3], [f64; 3], InputColorSpace); 3] = [
        (SRGB_R, SRGB_G, SRGB_B, InputColorSpace::Srgb),
        (P3_R, P3_G, P3_B, InputColorSpace::DisplayP3),
        (ADOBE_R, ADOBE_G, ADOBE_B, InputColorSpace::AdobeRgb),
    ];
    let mut best: Option<InputColorSpace> = None;
    let mut best_d = 1e9f64;
    for (rr, gg, bb, kind) in sets {
        let d = dist(r, rr) + dist(g, gg) + dist(b, bb);
        if d < best_d {
            best_d = d;
            best = Some(kind);
        }
    }
    if best_d < 0.02 {
        best
    } else {
        None
    }
}

/// 解析 ICC 的 RGB 基色标签（rXYZ/gXYZ/bXYZ），返回匹配的色彩空间（← iccToColorSpace）。
fn icc_to_color_space(icc: &[u8]) -> Option<InputColorSpace> {
    if icc.len() < 132 || &icc[36..40] != b"acsp" {
        return None;
    }
    // tag table
    let tag_count = u32(icc, 128) as usize;
    if tag_count == 0 || tag_count > 64 {
        return None;
    }
    let mut r: Option<[f64; 3]> = None;
    let mut g: Option<[f64; 3]> = None;
    let mut b: Option<[f64; 3]> = None;
    for i in 0..tag_count {
        let base = 132 + i * 12;
        if base + 12 > icc.len() {
            break;
        }
        let sig = &icc[base..base + 4];
        let off = u32(icc, base + 4) as usize;
        if off + 20 > icc.len() {
            continue;
        }
        let x = s15fixed(icc, off + 8);
        let y = s15fixed(icc, off + 12);
        let z = s15fixed(icc, off + 16);
        match sig {
            b"rXYZ" => r = Some([x, y, z]),
            b"gXYZ" => g = Some([x, y, z]),
            b"bXYZ" => b = Some([x, y, z]),
            _ => {}
        }
    }
    match (r, g, b) {
        (Some(r), Some(g), Some(b)) => match_primaries(r, g, b),
        _ => None,
    }
}

/// 从 JPEG 字节检测色彩空间（← detectJpeg；优先级 ICC > EXIF > JFIF(sRGB) > UNKNOWN）。
fn detect_jpeg(b: &[u8]) -> InputColorSpace {
    let mut off = 2usize;
    let mut has_jfif = false;
    let mut icc_match: Option<InputColorSpace> = None;
    let mut exif_match: Option<InputColorSpace> = None;
    while off + 4 <= b.len() {
        if b[off] != 0xFF {
            break;
        }
        let marker = u16(b, off);
        if marker == 0xFFDA || marker == 0xFFD9 {
            break; // SOS / EOI
        }
        let len = u16(b, off + 2);
        if len < 2 || off + 2 + len > b.len() {
            break;
        }
        let data = &b[off + 4..off + 2 + len];
        match marker {
            0xFFE0 => {
                // JFIF：无色彩声明，惯例 sRGB（不强制）
                if data.len() >= 5 && &data[..5] == b"JFIF" {
                    has_jfif = true;
                }
            }
            0xFFE1 => {
                // EXIF
                if data.len() >= 6 && &data[..4] == b"Exif" {
                    exif_match = parse_exif_color_space(data);
                }
            }
            0xFFE2 => {
                // ICC_PROFILE：payload = "ICC_PROFILE\0"(12) + seq(1) + total(1) + ICC 数据
                if data.len() >= 14 && &data[..12] == b"ICC_PROFILE\0" {
                    let icc = &data[14..];
                    icc_match = icc_to_color_space(icc);
                }
            }
            _ => {}
        }
        off += 2 + len;
    }
    icc_match.or(exif_match).or_else(|| {
        if has_jfif {
            Some(InputColorSpace::Srgb)
        } else {
            None
        }
    }).unwrap_or(InputColorSpace::Unknown)
}

/// 解析 EXIF 的 ColorSpace tag (0xA001)：1=sRGB, 2=AdobeRGB（← parseExifColorSpace）。
fn parse_exif_color_space(exif: &[u8]) -> Option<InputColorSpace> {
    // 跳过 "Exif\0\0"（6 字节）
    let t = 6usize;
    // TIFF 头：II/MM + 0x2A + IFD0 offset
    if t + 8 > exif.len() {
        return None;
    }
    let tiff = t;
    let mm = exif[t] == b'M' && exif[t + 1] == b'M';
    let ii = exif[t] == b'I' && exif[t + 1] == b'I';
    if !mm && !ii {
        return None;
    }
    let ifd0 = if mm {
        u32(exif, t + 4) as usize
    } else {
        // 小端
        (exif[t + 4] as usize)
            | ((exif[t + 5] as usize) << 8)
            | ((exif[t + 6] as usize) << 16)
            | ((exif[t + 7] as usize) << 24)
    };
    let p = tiff + ifd0;
    if p + 2 > exif.len() {
        return None;
    }
    let n_tags = if mm {
        u16(exif, p)
    } else {
        exif[p] as usize | ((exif[p + 1] as usize) << 8)
    };
    for i in 0..n_tags {
        let tp = p + 2 + i * 12;
        if tp + 12 > exif.len() {
            break;
        }
        let tag = if mm {
            u16(exif, tp)
        } else {
            exif[tp] as usize | ((exif[tp + 1] as usize) << 8)
        };
        if tag == 0xA001 {
            // 值在 offset 8（4 字节内联）
            let v = if mm {
                u32(exif, tp + 8)
            } else {
                (exif[tp + 8] as u32)
                    | ((exif[tp + 9] as u32) << 8)
                    | ((exif[tp + 10] as u32) << 16)
                    | ((exif[tp + 11] as u32) << 24)
            };
            return match v {
                1 => Some(InputColorSpace::Srgb),
                2 => Some(InputColorSpace::AdobeRgb),
                _ => None,
            };
        }
    }
    None
}

/// 从 PNG 字节检测色彩空间（← detectPng：sRGB/gAMA→SRGB，iCCP 不解压→UNKNOWN）。
fn detect_png(b: &[u8]) -> InputColorSpace {
    let mut p = 8usize;
    while p + 8 <= b.len() {
        let len = u32(b, p) as usize;
        let typ = &b[p + 4..p + 8];
        if typ == b"IEND" {
            break;
        }
        if typ == b"sRGB" {
            return InputColorSpace::Srgb;
        }
        if typ == b"gAMA" {
            return InputColorSpace::Srgb; // gamma 近似，多数 sRGB
        }
        if typ == b"iCCP" {
            // 与 Kotlin 一致：不解压，返回 UNKNOWN
            return InputColorSpace::Unknown;
        }
        p += 12 + len;
    }
    InputColorSpace::Unknown
}

/// 检测输入图像色彩空间（← `ColorSpaceDetector.detect`，行 215）。
pub fn detect(path: &Path) -> InputColorSpace {
    let b = read_head(path);
    if b.len() < 8 {
        return InputColorSpace::Unknown;
    }
    // PNG 签名：89 50 4E 47
    if b[0] == 0x89 && b[1] == b'P' && b[2] == b'N' && b[3] == b'G' {
        return detect_png(&b);
    }
    // JPEG 签名：FF D8
    if b[0] == 0xFF && b[1] == 0xD8 {
        return detect_jpeg(&b);
    }
    InputColorSpace::Unknown
}