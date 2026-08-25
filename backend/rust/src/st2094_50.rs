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