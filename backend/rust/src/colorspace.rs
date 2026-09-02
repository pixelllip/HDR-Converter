//! ← ColorSpaceDetector.kt：输入图像色彩空间探测。
//!
//! 依据 Ultra HDR 规范："SDR 图像的色彩配置定义了 HDR 图像的色彩空间"，
//! 不假设输入是 sRGB，先检测原图实际色彩空间（ICC / EXIF / JFIF / PNG 标记）。
//!
//! 检测顺序（与 Kotlin 一致）：ICC(APP2/iCCP) > EXIF ColorSpace > JFIF/PNG 标记 > UNKNOWN(默认 sRGB 假设)。
//!
//! 与 Kotlin 的分歧（Rust 侧扩展，Kotlin 已存档）：
//! - 输入色彩空间覆盖到 Rec.2020 / DCI-P3 / ProPhoto（不只是 sRGB / P3 / Adobe RGB）；
//! - PNG iCCP 不再"不解压 → UNKNOWN"：用 flate2 解压后走同一套 ICC 主色匹配；
//! - `detect` 额外返回**嵌入 ICC 字节**（原汤化原食：Ultra HDR 主图优先沿用原图 ICC，
//!   次选按检测空间程序化生成）。Kotlin 版只有枚举结果、无 ICC 字节。
//! - Rec.709 基色与 sRGB **完全相同**（BT.709 primaries == sRGB primaries），ICC 主色
//!   匹配无法区分两者 → 技术上归并到 Srgb（TRC 差异由主图 ICC 生成阶段区分，见 ultra_hdr.rs）。

use std::fmt;
use std::path::Path;

/// 输入图像色彩空间（扩展集：sRGB / Display-P3 / Adobe RGB / Rec.2020 / DCI-P3 / ProPhoto）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputColorSpace {
    Srgb,
    DisplayP3,
    AdobeRgb,
    Rec2020,
    DciP3,
    ProPhoto,
    Unknown,
}

impl fmt::Display for InputColorSpace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Srgb => "sRGB (IEC 61966-2-1)",
            Self::DisplayP3 => "Display-P3 (D65)",
            Self::AdobeRgb => "Adobe RGB (1998)",
            Self::Rec2020 => "Rec.2020 (BT.2020)",
            Self::DciP3 => "DCI-P3 (DCI white)",
            Self::ProPhoto => "ProPhoto RGB (ROMM)",
            Self::Unknown => "未声明（按 sRGB 假设）",
        };
        f.write_str(s)
    }
}

/// 探测结果：色彩空间 + 可选嵌入 ICC 字节（JPEG APP2 / PNG iCCP 原样保留）。
/// 供 Ultra HDR 主图"原汤化原食"使用：优先沿用原图 ICC，而非程序化生成。
#[derive(Debug, Clone)]
pub struct DetectedColorSpace {
    pub space: InputColorSpace,
    /// 原图中解出的 ICC profile 字节（若存在；未解析成功的返回 None）。
    pub embedded_icc: Option<Vec<u8>>,
}

// 已知基色（线性 XYZ，ICC s15Fixed16 归一化，rXYZ 标签值；← Kotlin 常量逐位照抄 + 扩展）
//
// Rec.2020 / DCI-P3 / ProPhoto 值取自 ICC 官方建议与常见 profile（lcms 生成值）：
// - Rec.2020 主色 xy：R(0.708,0.292) G(0.170,0.797) B(0.131,0.046)，白 D65；D50 适配 rXYZ 见下
// - DCI-P3 主色 xy 与 Display-P3 相同，但白点不同（DCI 白 0.314,0.351）；D50 适配后 rXYZ 有差异
// - ProPhoto（ROMM）：R(0.7347,0.2653) G(0.1596,0.8404) B(0.0366,0.0001)，白点 D50(0.3457,0.3585)
const SRGB_R: [f64; 3] = [0.436041, 0.222485, 0.013920];
const P3_R: [f64; 3] = [0.515102, 0.241186, -0.001126];
const ADOBE_R: [f64; 3] = [0.609740, 0.205940, 0.149190];
const REC2020_R: [f64; 3] = [0.673462, 0.279038, -0.001938];
const DCIP3_R: [f64; 3] = [0.515120, 0.239993, -0.001161];
const PROPHOTO_R: [f64; 3] = [0.797766, 0.288041, 0.000000];
const SRGB_G: [f64; 3] = [0.385113, 0.716879, 0.097109];
const P3_G: [f64; 3] = [0.291979, 0.692219, 0.041882];
const ADOBE_G: [f64; 3] = [0.311110, 0.625710, 0.063250];
const REC2020_G: [f64; 3] = [0.165665, 0.675339, 0.029984];
const DCIP3_G: [f64; 3] = [0.287340, 0.687020, 0.040835];
const PROPHOTO_G: [f64; 3] = [0.288041, 0.712137, 0.000014];
const SRGB_B: [f64; 3] = [0.143051, 0.060608, 0.713913];
const P3_B: [f64; 3] = [0.157101, 0.066593, 0.784072];
const ADOBE_B: [f64; 3] = [0.125710, 0.070240, 0.991050];
const REC2020_B: [f64; 3] = [0.125107, 0.045624, 0.797165];
const DCIP3_B: [f64; 3] = [0.156957, 0.066161, 0.783875];
const PROPHOTO_B: [f64; 3] = [0.135126, 0.035061, 0.723524];

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

/// 基色匹配：比较三个主色（忽略微小差异），返回最接近的已知空间（阈值 0.02）。
fn match_primaries(r: [f64; 3], g: [f64; 3], b: [f64; 3]) -> Option<InputColorSpace> {
    fn dist(a: [f64; 3], reference: [f64; 3]) -> f64 {
        let mut d = 0.0;
        for i in 0..3 {
            d += (a[i] - reference[i]) * (a[i] - reference[i]);
        }
        d.sqrt()
    }
    let sets: [([f64; 3], [f64; 3], [f64; 3], InputColorSpace); 6] = [
        (SRGB_R, SRGB_G, SRGB_B, InputColorSpace::Srgb),
        (P3_R, P3_G, P3_B, InputColorSpace::DisplayP3),
        (ADOBE_R, ADOBE_G, ADOBE_B, InputColorSpace::AdobeRgb),
        (REC2020_R, REC2020_G, REC2020_B, InputColorSpace::Rec2020),
        (DCIP3_R, DCIP3_G, DCIP3_B, InputColorSpace::DciP3),
        (PROPHOTO_R, PROPHOTO_G, PROPHOTO_B, InputColorSpace::ProPhoto),
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
fn match_icc_primaries(icc: &[u8]) -> Option<InputColorSpace> {
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

/// 校验 ICC 是否为合法 RGB 显示 profile（acsp 签名 + RGB 色彩空间），供"原汤化原食"沿用原 ICC。
fn is_valid_rgb_icc(icc: &[u8]) -> bool {
    icc.len() >= 132
        && &icc[36..40] == b"acsp"
        && icc.len() >= 20
        && &icc[16..20] == b"RGB "
}

/// 从 JPEG 字节检测色彩空间（优先级 ICC > EXIF > JFIF(sRGB) > UNKNOWN），并保留嵌入 ICC 字节。
fn detect_jpeg(b: &[u8]) -> (InputColorSpace, Option<Vec<u8>>) {
    let mut off = 2usize;
    let mut has_jfif = false;
    let mut icc_match: Option<InputColorSpace> = None;
    let mut icc_bytes: Option<Vec<u8>> = None;
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
                    // 取首个分段（常见单段）；多段时逐段累计
                    let chunk = &data[14..];
                    let full = match &mut icc_bytes {
                        Some(acc) => {
                            // 追加（预留：多数文件单段，直接替换即可满足主色匹配与沿用）
                            acc.extend_from_slice(chunk);
                            acc
                        }
                        None => {
                            icc_bytes = Some(chunk.to_vec());
                            icc_bytes.as_mut().unwrap()
                        }
                    };
                    if icc_match.is_none() {
                        icc_match = match_icc_primaries(full);
                    }
                    // 即便是多段，段内也可尝试匹配（icc_bytes 会越拼越完整）
                    if icc_match.is_none() {
                        icc_match = match_icc_primaries(full);
                    }
                }
            }
            _ => {}
        }
        off += 2 + len;
    }
    let space = icc_match.or(exif_match).or_else(|| {
        if has_jfif {
            Some(InputColorSpace::Srgb)
        } else {
            None
        }
    }).unwrap_or(InputColorSpace::Unknown);
    // 嵌入 ICC：仅当它是合法 RGB 显示 profile 时保留（沿用给 Ultra HDR 主图）
    let embedded = icc_bytes.filter(|icc| is_valid_rgb_icc(icc));
    (space, embedded)
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

/// 从 PNG 字节检测色彩空间与嵌入 ICC：
/// - iCCP：用 flate2 解压出 ICC → 主色匹配 + 保留 ICC 字节（Kotlin 原为直接返回 UNKNOWN，Rust 已扩展）
/// - sRGB chunk → Srgb；gAMA → Srgb 兜底（近似，多数 sRGB；仅当无更明确的 ICC/iCCP 结论时生效）
fn detect_png(b: &[u8]) -> (InputColorSpace, Option<Vec<u8>>) {
    let mut p = 8usize;
    let mut icc_bytes: Option<Vec<u8>> = None;
    let mut icc_match: Option<InputColorSpace> = None;
    let mut saw_gama = false;
    let mut direct: Option<InputColorSpace> = None;
    while p + 8 <= b.len() {
        let len = u32(b, p) as usize;
        let typ = &b[p + 4..p + 8];
        if typ == b"IEND" {
            break;
        }
        if typ == b"sRGB" {
            // sRGB chunk 是强信号，但仍继续扫描 iCCP（若后续有 iCCP 以 ICC 为准）
            if direct.is_none() {
                direct = Some(InputColorSpace::Srgb);
            }
        }
        if typ == b"gAMA" {
            saw_gama = true;
        }
        if typ == b"iCCP" && p + 12 + len <= b.len() {
            let chunk = &b[p + 8..p + 8 + len];
            // iCCP: profile_name(1..79, \0 结尾) + 压缩方法(1) + 压缩数据(zlib)
            let name_end = chunk.iter().position(|&c| c == 0).unwrap_or(chunk.len().min(79));
            let comp_method = chunk.get(name_end + 1).copied();
            // 仅支持 zlib（压缩方法 0）
            if comp_method == Some(0) {
                let data = &chunk[name_end + 2..];
                use std::io::Read as _;
                let mut d = flate2::read::ZlibDecoder::new(data);
                let mut icc = Vec::with_capacity(data.len() * 2);
                if d.read_to_end(&mut icc).is_ok() && icc.len() >= 132 && &icc[36..40] == b"acsp" {
                    icc_bytes = Some(icc.clone());
                    icc_match = match_icc_primaries(&icc);
                }
            }
        }
        p += 12 + len;
    }
    // 优先级：iCCP 主色匹配 > sRGB chunk > gAMA 兜底 > UNKNOWN
    let space = icc_match
        .or(direct)
        .or_else(|| {
            if saw_gama {
                Some(InputColorSpace::Srgb)
            } else {
                None
            }
        })
        .unwrap_or(InputColorSpace::Unknown);
    (space, icc_bytes.filter(|icc| is_valid_rgb_icc(icc)))
}

/// 检测输入图像色彩空间（← `ColorSpaceDetector.detect`）。
/// 返回空间 + 可选嵌入 ICC 字节（原汤化原食：Ultra HDR 主图可沿用原图 ICC）。
pub fn detect(path: &Path) -> DetectedColorSpace {
    let b = read_head(path);
    if b.len() < 8 {
        return DetectedColorSpace { space: InputColorSpace::Unknown, embedded_icc: None };
    }
    // PNG 签名：89 50 4E 47
    if b[0] == 0x89 && b[1] == b'P' && b[2] == b'N' && b[3] == b'G' {
        let (space, embedded_icc) = detect_png(&b);
        return DetectedColorSpace { space, embedded_icc };
    }
    // JPEG 签名：FF D8
    if b[0] == 0xFF && b[1] == 0xD8 {
        let (space, embedded_icc) = detect_jpeg(&b);
        return DetectedColorSpace { space, embedded_icc };
    }
    DetectedColorSpace { space: InputColorSpace::Unknown, embedded_icc: None }
}