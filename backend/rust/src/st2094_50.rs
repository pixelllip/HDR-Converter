//! ← st2094_50.js：SMPTE ST 2094-50（Application #5 / Eclipsa Video）最小实现。
//!
//! 参考白配方（C.3.8，use_reference_white_tone_mapping=1）编码 + ITU-T T.35 载荷 +
//! HEVC Prefix_SEI NAL（user_data_registered_itu_t_t35，payload_type=4，含 EBSP 转义）。
//!
//! Application #5 载荷结构（逐位对齐 JS）：
//!   application_info       = [version_flags(0x00)] + color_volume_transform
//!   color_volume_transform = [0x40(hasAdaptiveToneMap)] + adaptive_tone_map
//!   adaptive_tone_map      = u16(baselineHdrHeadroom) + [0x80(useReferenceWhite=1)]
//!   t35                    = [B5 00 90 00 01] + application_info
//!   sei_rbsp               = [04] + size… + t35 + [80]
//!   nal                    = [4E 01] + rbsp（EBSP 转义后）

/// ITU-T T.35 / SMPTE 常量（← JS T35_*）。
pub const T35_COUNTRY_US: u8 = 0xB5;
pub const T35_PROVIDER_SMPTE: u16 = 0x0090;
pub const T35_ORIENTED_APP5: u16 = 0x0001;

/// ← `encodeApplicationInfo(vectorReferenceWhiteRecipe(raw))`：
/// 参考白配方 application_info 字节（baselineHdrHeadroom 为 raw u16，×10000 缩放由上层负责）。
pub fn reference_white_app_info(baseline_hdr_headroom: u16) -> Vec<u8> {
    let mut v = vec![0x00, 0x40];
    v.extend_from_slice(&baseline_hdr_headroom.to_be_bytes());
    v.push(0x80);
    v
}

/// ← `t35Payload`：加 T.35 前缀（B5 00 90 00 01）。
pub fn t35_payload(app_info: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(5 + app_info.len());
    v.push(T35_COUNTRY_US);
    v.push((T35_PROVIDER_SMPTE >> 8) as u8);
    v.push((T35_PROVIDER_SMPTE & 0xFF) as u8);
    v.push((T35_ORIENTED_APP5 >> 8) as u8);
    v.push((T35_ORIENTED_APP5 & 0xFF) as u8);
    v.extend_from_slice(app_info);
    v
}

/// ← `seiRbsp`：一条 payload_type=4 的 T.35 消息 + rbsp_stop_one_bit（0x80）。
fn sei_rbsp(t35: &[u8]) -> Vec<u8> {
    let mut v = vec![0x04];
    let mut size = t35.len();
    while size >= 0xFF {
        v.push(0xFF);
        size -= 0xFF;
    }
    v.push(size as u8);
    v.extend_from_slice(t35);
    v.push(0x80);
    v
}

/// ← `ebspNal`：EBSP 转义（00 00 00/01/02/03 → 插入 0x03），作用于整个 NAL（含头）。
fn ebsp_escape(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 16);
    let mut zeros = 0u8;
    for &b in data {
        if zeros >= 2 && b <= 0x03 {
            out.push(0x03);
            zeros = 0;
        }
        out.push(b);
        zeros = if b == 0 { zeros + 1 } else { 0 };
    }
    out
}

/// ← `buildPrefixSeiNal`：HEVC Prefix_SEI NAL（nal_unit_type=39 → 头 0x4E 0x01）。
pub fn build_prefix_sei_nal(t35: &[u8]) -> Vec<u8> {
    let mut nal = vec![0x4E, 0x01];
    nal.extend_from_slice(&sei_rbsp(t35));
    ebsp_escape(&nal)
}

/// 便捷向量：参考白配方 → 完整 Prefix_SEI NAL 字节（供按帧注入）。
pub fn reference_white_prefix_sei(baseline_hdr_headroom: u16) -> Vec<u8> {
    build_prefix_sei_nal(&t35_payload(&reference_white_app_info(baseline_hdr_headroom)))
}

/// PQ EOTF（码值 0..1 → 尼特；← JS pqEotf）。
pub fn pq_eotf(v01: f64) -> f64 {
    let m = 78.84375;
    let n = 0.1593017578125;
    let c1 = 0.8359375;
    let c2 = 18.8515625;
    let c3 = 18.6875;
    let y = v01.clamp(0.0, 1.0).powf(1.0 / m);
    ((y - c1).max(0.0) / (c2 - c3 * y)).powf(1.0 / n) * 10000.0
}

// ===========================================================================
// Annex C **解析**侧（读 2094-50）—— 位级语义对齐 hdr-explorer app/agtm_parser.ts
// `parseAgtm()`（AGTM = ST 2094-50），输出可用于「HDR 视频编辑器」与
// 「Headroom 预览面板」的结构化数据。
// 输入 = application_info 起始字节（T.35 头由上层剥离，见 parse_t35_payload）。
// ===========================================================================

use serde::Serialize;

/// 标准色度坐标 [rx, ry, gx, gy, bx, by, wx, wy]（mode 0/1/2 常量，D65 白点）。
/// mode 3（自定义）时从码流读取 8×u16/50000。
const CHROMA_SRGB: [f64; 8] = [
    0.640, 0.330, 0.300, 0.600, 0.150, 0.060, 0.3127, 0.3290,
];
const CHROMA_P3: [f64; 8] = [
    0.680, 0.320, 0.265, 0.690, 0.150, 0.060, 0.3127, 0.3290,
];
const CHROMA_REC2020: [f64; 8] = [
    0.708, 0.292, 0.170, 0.797, 0.131, 0.046, 0.3127, 0.3290,
];

/// CICP colour_primaries 枚举（gain_application_space 常量输出用）。
pub const CICP_PRIMARIES_SRGB: u8 = 1;
pub const CICP_PRIMARIES_P3: u8 = 12;
pub const CICP_PRIMARIES_REC2020: u8 = 9;

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Point2 {
    pub x: f64,
    pub y: f64,
    /// 斜率 m（PCHIP 拟合或 θ=tan 显式给出）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub m: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct ComponentMix {
    /// r/g/b 通道权重，和须 ≤ 1（与 max/min/channel 权重归一）。
    pub rgb: [f64; 3],
    /// maxRGB 权重。
    pub max: f64,
    /// minRGB 权重。
    pub min: f64,
    /// channel 权重（c = dot(rgb, in) 后再叠加 max/min）。
    pub channel: f64,
}

impl ComponentMix {
    pub fn max_only() -> Self {
        Self { rgb: [0.0; 3], max: 1.0, min: 0.0, channel: 0.0 }
    }
}

/// 「Alternative 图像色调映射规则」：一条面向某示器 headroom 的增益曲线。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Altr {
    /// 目标 headroom（log2 单位）。
    pub headroom: f64,
    /// ≤ 32 个控制点的增益曲线（x ∈ [0,64]，y ∈ [-6,6]，m 斜率）。
    pub curve: Vec<Point2>,
    pub mix: ComponentMix,
}

/// AGTM（SMPTE ST 2094-50）结构化元数据。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AgtmMetadata {
    /// 0..=4 条 alternate；隐式「零增益」曲线不在此列。
    pub altr: Vec<Altr>,
    /// gain application 色域（CICP 枚举）或自定义色度。
    pub gain_application_space_primaries: Option<u8>,
    pub gain_application_space_chromaticities: Option<[f64; 8]>,
    /// HDR 参考白（SDR 白点，nit）。
    pub hdr_reference_white: f64,
    /// 基准 headroom（log2 单位）。
    pub baseline_hdr_headroom: f64,
}

/// 逐位读取器（MSB-first，与 hdr-explorer Bitstream 语义一致）。
pub struct BitReader<'a> {
    data: &'a [u8],
    bit_pos: usize,
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, bit_pos: 0 }
    }

    /// 读 1 bit；耗尽返回 None。
    #[inline]
    pub fn read_bit(&mut self) -> Option<u8> {
        let byte = self.bit_pos / 8;
        if byte >= self.data.len() {
            return None;
        }
        let bit = 7 - (self.bit_pos % 8);
        self.bit_pos += 1;
        Some((self.data[byte] >> bit) & 1)
    }

    /// 读 n bit（n ≤ 32）；位数不足返回 None。
    pub fn read_bits(&mut self, n: u8) -> Option<u32> {
        if n > 32 {
            return None;
        }
        let mut v: u32 = 0;
        for _ in 0..n {
            v = (v << 1) | self.read_bit()? as u32;
        }
        Some(v)
    }

    /// 跳过 n bit；位数不足则整体失败。
    pub fn skip_bits(&mut self, n: u8) -> Option<()> {
        for _ in 0..n {
            self.read_bit()?;
        }
        Some(())
    }

    pub fn remaining(&self) -> usize {
        self.data.len() * 8 - self.bit_pos
    }
}

/// 读 u16 并按 (v - shift) / scale 缩放，clamp 到 [min, max]（← JS readScaledU16）。
/// 位数不足返回 None（截断视为解析失败）。
fn read_scaled_u16(r: &mut BitReader, scale: f64, shift: f64, min: f64, max: f64) -> Option<f64> {
    let raw = r.read_bits(16)? as f64;
    Some((raw.clamp(min, max) - shift) / scale)
}

/// 解析 gain application 色度：mode 0/1/2 常量，3 从码流读 8×u16/50000。
fn parse_chromaticities(
    r: &mut BitReader,
    mode: u32,
) -> Option<(Option<u8>, Option<[f64; 8]>)> {
    match mode {
        0 => Some((Some(CICP_PRIMARIES_SRGB), Some(CHROMA_SRGB))),
        1 => Some((Some(CICP_PRIMARIES_P3), Some(CHROMA_P3))),
        2 => Some((Some(CICP_PRIMARIES_REC2020), Some(CHROMA_REC2020))),
        3 => {
            let mut c = [0.0f64; 8];
            for v in c.iter_mut() {
                *v = read_scaled_u16(r, 50000.0, 0.0, 0.0, 50000.0)?;
            }
            Some((None, Some(c)))
        }
        _ => None,
    }
}

/// 解析 smpte_st_2094_50_component_mixing() → ComponentMix（含权重归一）。
fn parse_component_mixing(r: &mut BitReader) -> Option<ComponentMix> {
    let component_mixing_type = r.read_bits(2)?;
    let mut mix = match component_mixing_type {
        0 => ComponentMix::max_only(),
        1 => ComponentMix { rgb: [0.0; 3], max: 0.0, min: 0.0, channel: 1.0 },
        2 => ComponentMix { rgb: [1.0 / 6.0; 3], max: 0.5, min: 0.0, channel: 0.0 },
        3 => {
            // 6 个 has-present 标志 + 按标志读 6 个系数（r/g/b/max/min/channel）。
            let mut coeffs = [0.0f64; 6];
            let mut present = [false; 6];
            for (i, p) in present.iter_mut().enumerate() {
                *p = r.read_bit()? == 1;
                if *p {
                    coeffs[i] = read_scaled_u16(r, 50000.0, 0.0, 0.0, 50000.0)?;
                }
            }
            ComponentMix {
                rgb: [coeffs[0], coeffs[1], coeffs[2]],
                max: coeffs[3],
                min: coeffs[4],
                channel: coeffs[5],
            }
        }
        _ => return None,
    };
    if component_mixing_type != 3 {
        r.skip_bits(6)?; // reserved
    }
    // 权重归一化（和 > 0 时）。
    let weight_sum =
        mix.rgb[0] + mix.rgb[1] + mix.rgb[2] + mix.max + mix.min + mix.channel;
    if weight_sum > 0.0 {
        mix.rgb[0] /= weight_sum;
        mix.rgb[1] /= weight_sum;
        mix.rgb[2] /= weight_sum;
        mix.max /= weight_sum;
        mix.min /= weight_sum;
        mix.channel /= weight_sum;
    }
    Some(mix)
}

/// 解析 smpte_st_2094_50_gain_curve() → 控制点数组（x 已知，y/m 后续填充）+ pchip 标志。
/// `curve_prev`：common-curve 时复用上一 alternate 的 x/ncp/pchip。
fn parse_gain_curve(r: &mut BitReader, curve_prev: Option<&Altr>) -> Option<(Vec<Point2>, bool)> {
    if let Some(prev) = curve_prev {
        let xs: Vec<f64> = prev.curve.iter().map(|p| p.x).collect();
        let pchip = prev.curve.first().is_some_and(|p| p.m.is_some());
        let curve = xs.into_iter().map(|x| Point2 { x, y: 0.0, m: None }).collect();
        return Some((curve, pchip));
    }
    let ncp = r.read_bits(5)? as usize + 1;
    let pchip = r.read_bit()? == 1;
    r.skip_bits(2)?; // reserved
    let mut curve = Vec::with_capacity(ncp);
    for _ in 0..ncp {
        curve.push(Point2 { x: read_scaled_u16(r, 1000.0, 0.0, 0.0, 64000.0)?, y: 0.0, m: None });
    }
    Some((curve, pchip))
}

/// 解析 smpte_st_2094_50_adaptive_tone_map 通用分支 + C.3.8 参考白配方分支。
///
/// 输入 = adaptive_tone_map 起始字节。C.3.8 分支（use_reference_white_tone_mapping=1）
/// 不读额外字节，按 baseline 用公式合成两条 ALTR（← hdr-explorer agtm_parser.ts
/// `parseAgtm` useReferenceWhiteToneMapping 分支）。
fn parse_adaptive_tone_map(r: &mut BitReader) -> Option<AgtmMetadata> {
    let baseline_hdr_headroom = read_scaled_u16(r, 10000.0, 0.0, 0.0, 60000.0)?;
    // 1 字节标志：bit7=useRefWhite | bits6-4=numAltr | bits3-2=chromaMode | bit1=commonMix | bit0=commonCurve
    let flags = r.read_bits(8)?;
    let use_reference_white_tone_mapping = (flags >> 7) & 1 == 1;
    if use_reference_white_tone_mapping {
        // C.3.8：由 baseline 公式合成 2 条 ALTR（headroom=0 与 headroom=log2(8/3)·t）。
        let t = (baseline_hdr_headroom / (1000.0_f64 / 203.0).log2()).clamp(0.0, 1.0);
        let mut altr = Vec::with_capacity(2);
        for i in 0..2 {
            let headroom = if i == 0 { 0.0 } else { (8.0_f64 / 3.0).log2() * t };
            let y_white = if i == 0 { 1.0 - 0.5 * t } else { 1.0 };
            let kappa = 0.65;
            let x_knee = 1.0;
            let y_knee = y_white;
            let x_max = 2.0f64.powf(baseline_hdr_headroom);
            let y_max = 2.0f64.powf(headroom);
            let x_mid = (1.0 - kappa) * x_knee + (kappa * x_knee * y_max) / y_knee;
            let y_mid = (1.0 - kappa) * y_knee + kappa * y_max;
            let (xa, ya) = (x_knee - 2.0 * x_mid + x_max, y_knee - 2.0 * y_mid + y_max);
            let (xb, yb) = (2.0 * x_mid - 2.0 * x_knee, 2.0 * y_mid - 2.0 * y_knee);
            let (xc, yc) = (x_knee, y_knee);
            let mut curve = Vec::with_capacity(8);
            for c in 0..8usize {
                let t = c as f64 / 7.0;
                let x = xc + t * (xb + t * xa);
                let y = yc + t * (yb + t * ya);
                let m = (2.0 * ya * t + yb) / (2.0 * xa * t + xb);
                curve.push(Point2 {
                    x,
                    y: (y / x).log2(),
                    m: Some((x * m - y) / (std::f64::consts::LN_2 * x * y)),
                });
            }
            altr.push(Altr { headroom, curve, mix: ComponentMix::max_only() });
        }
        return Some(AgtmMetadata {
            altr,
            gain_application_space_primaries: Some(CICP_PRIMARIES_REC2020),
            gain_application_space_chromaticities: Some(CHROMA_REC2020),
            hdr_reference_white: 203.0, // 由调用方按 hasCustomHdrReferenceWhiteFlag 覆盖
            baseline_hdr_headroom,
        });
    }

    // 通用分支。
    let num_altr = ((flags >> 4) & 0x07) as usize;
    let chromaticities_mode = (flags >> 2) & 0x03;
    let has_common_mix = (flags >> 1) & 1 == 1;
    let has_common_curve = flags & 1 == 1;
    let (primaries, chromaticities) =
        parse_chromaticities(r, chromaticities_mode)?;

    let mut altr = Vec::with_capacity(num_altr.min(4));
    let mut common_mix: Option<ComponentMix> = None;
    let mut common_curve: Option<Vec<Point2>> = None;
    let mut common_pchip = false;
    for i in 0..num_altr.min(4) {
        let headroom = read_scaled_u16(r, 10000.0, 0.0, 0.0, 60000.0)?;
        let mix = if i == 0 || !has_common_mix {
            let m = parse_component_mixing(r)?;
            if i == 0 {
                common_mix = Some(m);
            }
            m
        } else {
            common_mix.unwrap_or_else(ComponentMix::max_only)
        };
        let (curve, pchip) = if i == 0 || !has_common_curve {
            let prev = common_curve.as_ref().map(|c| Altr {
                headroom: 0.0,
                curve: c.clone(),
                mix: ComponentMix::max_only(),
            });
            let (c, p) = parse_gain_curve(r, prev.as_ref())?;
            if i == 0 {
                common_curve = Some(c.clone());
                common_pchip = p;
            }
            (c, p)
        } else {
            (common_curve.clone().unwrap_or_default(), common_pchip)
        };
        // 读 y（每条 ALTR 独立），符号 = baseline 与 headroom 比较。
        let sign = if baseline_hdr_headroom < headroom { 1.0 } else { -1.0 };
        let mut curve = curve;
        for p in curve.iter_mut() {
            p.y = read_scaled_u16(r, 10000.0, 0.0, 0.0, 60000.0)? * sign;
        }
        // 斜率：pchip=0 时显式读 θ→tan；pchip=1 时留空（由上层按控制点重算，
        // 与 hdr-explorer piecewise_cubic / pchip 一致）。
        if pchip {
            for p in curve.iter_mut() {
                p.m = None;
            }
        } else {
            for p in curve.iter_mut() {
                let theta = read_scaled_u16(
                    r,
                    36000.0 / std::f64::consts::PI,
                    18000.0,
                    1.0,
                    35999.0,
                )?;
                p.m = Some(theta.tan());
            }
        }
        altr.push(Altr { headroom, curve, mix });
    }

    Some(AgtmMetadata {
        altr,
        gain_application_space_primaries: primaries,
        gain_application_space_chromaticities: chromaticities,
        hdr_reference_white: 203.0,
        baseline_hdr_headroom,
    })
}

/// 解析 smpte_st_2094_50_color_volume_transform()。
/// 返回 (hdr_reference_white 是否自定义值, hdr_reference_white, 是否含 adaptive_tone_map)。
fn parse_color_volume_transform(
    r: &mut BitReader,
    mut meta: AgtmMetadata,
) -> Option<AgtmMetadata> {
    let f = r.read_bits(8)?;
    let has_custom_hdr_reference_white = (f >> 7) & 1 == 1;
    let has_adaptive_tone_map = (f >> 6) & 1 == 1;
    if has_custom_hdr_reference_white {
        meta.hdr_reference_white = read_scaled_u16(r, 5.0, 0.0, 1.0, 50000.0)?;
    }
    if has_adaptive_tone_map {
        let atm = parse_adaptive_tone_map(r)?;
        return Some(AgtmMetadata {
            hdr_reference_white: if has_custom_hdr_reference_white {
                meta.hdr_reference_white
            } else {
                203.0
            },
            ..atm
        });
    }
    Some(meta)
}

/// 完整解析 application_info（不含 T.35 头）。
/// 返回 None 表示非法/截断载荷。
pub fn parse_application_info(payload: &[u8]) -> Option<AgtmMetadata> {
    let mut r = BitReader::new(payload);
    let version_flags = r.read_bits(8)?;
    let application_version = (version_flags >> 5) & 0x07;
    let minimum_application_version = (version_flags >> 2) & 0x07;
    // 语法上：application_version >= minimum；本解析器仅支持版本 0（与 JS/Rust 编码侧一致）。
    if application_version != 0 || minimum_application_version > 0 {
        return None;
    }
    let base = AgtmMetadata {
        altr: Vec::new(),
        gain_application_space_primaries: None,
        gain_application_space_chromaticities: None,
        hdr_reference_white: 203.0,
        baseline_hdr_headroom: 0.0,
    };
    parse_color_volume_transform(&mut r, base)
}

/// 解析完整 ITU-T T.35 载荷（country 0xB5 / provider 0x0090 / oriented 0x0001 + appInfo）。
/// 头不匹配时返回 Ok(None)（不是 2094-50，可能是 HDR10+ 等其它 T.35 应用）。
pub fn parse_t35_payload(payload: &[u8]) -> Result<Option<AgtmMetadata>, String> {
    if payload.len() < 5 {
        return Err("T.35 载荷过短".into());
    }
    if payload[0] != T35_COUNTRY_US
        || (u16::from(payload[1]) << 8 | u16::from(payload[2])) != T35_PROVIDER_SMPTE
        || (u16::from(payload[3]) << 8 | u16::from(payload[4])) != T35_ORIENTED_APP5
    {
        return Ok(None);
    }
    match parse_application_info(&payload[5..]) {
        Some(meta) => Ok(Some(meta)),
        None => Err("application_info 解析失败（非法/截断）".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// C.3.8 参考白配方 round-trip：encode → parse → 语义校验。
    #[test]
    fn reference_white_recipe_roundtrip() {
        let raw: u16 = 14691; // Hbaseline = 1.4691 档（×10000）
        let app_info = reference_white_app_info(raw);
        let t35 = t35_payload(&app_info);
        let meta = parse_t35_payload(&t35).expect("解析失败").expect("应为 2094-50");

        assert!((meta.baseline_hdr_headroom - 1.4691).abs() < 1e-9);
        assert_eq!(meta.hdr_reference_white, 203.0);
        assert_eq!(meta.altr.len(), 2);
        assert_eq!(meta.altr[0].headroom, 0.0);
        // 第二条 headroom = log2(8/3) * min(baseline/log2(1000/203), 1)
        let t = (1.4691_f64 / (1000.0_f64 / 203.0).log2()).clamp(0.0, 1.0);
        assert!((meta.altr[1].headroom - (8.0_f64 / 3.0).log2() * t).abs() < 1e-9);
        // 每条 8 个控制点，x 递增（0..x_max）。
        assert_eq!(meta.altr[0].curve.len(), 8);
        assert_eq!(meta.altr[1].curve.len(), 8);
        assert!(meta.altr[1].curve.iter().all(|p| p.m.is_some()));
        // 参考白配方强制 Rec.2020 增益色域。
        assert_eq!(meta.gain_application_space_primaries, Some(CICP_PRIMARIES_REC2020));
    }

    /// 非 2094-50 的 T.35（如 HDR10+ provider 0x003C）→ Ok(None)。
    #[test]
    fn t35_other_provider_is_none() {
        let mut t35 = vec![0xB5, 0x00, 0x3C, 0x00, 0x01];
        t35.extend_from_slice(&[0u8; 16]);
        assert!(parse_t35_payload(&t35).expect("不应是 Err").is_none());
    }

    /// 截断载荷 → Err。
    #[test]
    fn truncated_payload_is_err() {
        let t35 = vec![0xB5, 0x00, 0x90, 0x00]; // 缺 1 字节头
        assert!(parse_t35_payload(&t35).is_err());
    }

    /// 通用分支（显式 ALTR）：手写 application_info 向量 → 语义校验。
    /// 构造：numAltr=1, chromaMode=0(sRGB), commonMix=0, commonCurve=0,
    ///       mix type=0(max), curve ncp=3 pchip=0（显式 θ）, x=[0,1000,2000],
    ///       y=[0,5000,10000], baseline=20000(2.0 档) → headroom=0 < baseline → sign=-1 → y 反号。
    #[test]
    fn explicit_altr_parse() {
        // application_info = version_flags(0x00) + cvt(0x40: hasAdaptiveToneMap)
        //   + baseline u16(0x4E20=20000) + flags(0x10: numAltr=1<<4 | chroma0 | mix0 | curve0)
        //   + altr0 headroom u16(0x0000) + mix(0x00 type0+reserved) + curve(0x10: (ncp-1=2)<<3 | pchip=0)
        //   + x: 0000 03E8 07D0 + y: 0000 1388 2710 + theta: 3×0x4E20
        let mut v: Vec<u8> = vec![0x00, 0x40, 0x4E, 0x20, 0x10, 0x00, 0x00, 0x00, 0x10];
        v.extend_from_slice(&[0x00, 0x00, 0x03, 0xE8, 0x07, 0xD0]); // x
        v.extend_from_slice(&[0x00, 0x00, 0x13, 0x88, 0x27, 0x10]); // y
        v.extend_from_slice(&[0x4E, 0x20, 0x4E, 0x20, 0x4E, 0x20]); // theta(3)
        let meta = parse_application_info(&v).expect("解析失败");

        assert_eq!(meta.baseline_hdr_headroom, 2.0);
        assert_eq!(meta.altr.len(), 1);
        let a0 = &meta.altr[0];
        assert_eq!(a0.headroom, 0.0);
        assert!((a0.mix.max - 1.0).abs() < 1e-12); // type 0 = max-only
        assert_eq!(a0.curve.len(), 3);
        assert!((a0.curve[2].x - 2.0).abs() < 1e-9); // 2000/1000
        // baseline(2.0) > headroom(0) → sign=-1 → y 反号
        assert!((a0.curve[2].y - -1.0).abs() < 1e-9); // 10000/10000 * -1
        // 显式 θ → m = tan（0x4E20 → θ≈π/18 rad → tan > 0）
        assert!(a0.curve[0].m.unwrap() > 0.0);
        // chromaMode=0 → sRGB 常量
        assert_eq!(meta.gain_application_space_primaries, Some(CICP_PRIMARIES_SRGB));
    }
}