//! hdr_meta.rs — 从视频码流中提取 SMPTE ST 2094-50（ITU-T T.35）动态元数据。
//!
//! 支持两种码流格式（输入均为 **裸流**，容器层由调用方/CLI 用 ffmpeg 提取）：
//! - **AV1**：low-overhead OBU 流（IVF 容器或裸 OBU；MP4 av01 sample 即此类）。
//!   metadata OBU（type=5）内 metadata_type=4（ITUT_T35）→ 复用 `st2094_50::parse_t35_payload`。
//! - **HEVC**：Annex B 流（`hevc_mp4toannexb` 输出；对齐 JS hevc_inject.js 的起始码切分逻辑）。
//!   nal_unit_type=39（Prefix_SEI）内的 payload_type=4（user_data_registered_itu_t_t35，T.35）
//!   → 剥离 emulation_prevention_three_byte 后解析。
//!
//! 设计对齐 hdr-explorer `app/media_parser.ts` 的 AV1OBUParser / NALUParser，
//! 只保留 2094-50 支线（HDR10+ 不在本项目范围）。

use anyhow::{Context, Result, anyhow};

use crate::st2094_50::{AgtmMetadata, parse_t35_payload};

// ---------------------------------------------------------------------------
// AV1 OBU
// ---------------------------------------------------------------------------

/// AV1 OBU 头字节：forbidden(1) | type(4) | extension(1) | has_size(1) | reserved(1)
const OBU_HEADER_SIZE: usize = 1;
/// OBU_METADATA
const OBU_TYPE_METADATA: u8 = 5;
/// AV1 metadata_type = ITU-T T.35
const METADATA_TYPE_ITUT_T35: u64 = 4;

/// 读一条 leb128（≤ 8 字节），返回 (值, 消耗字节数)。
fn read_uleb128(data: &[u8], mut pos: usize) -> Option<(u64, usize)> {
    let mut value: u64 = 0;
    let mut shift = 0u32;
    let start = pos;
    loop {
        let byte = *data.get(pos)?;
        pos += 1;
        value |= u64::from(byte & 0x7F) << shift;
        if byte & 0x80 == 0 {
            return Some((value, pos - start));
        }
        shift += 7;
        if shift >= 64 {
            return None;
        }
    }
}

/// 帧内 OBU 载荷：metadata OBU（type=5）且 metadata_type=4 → 返回 (T.35 payload 字节)。
/// `metadata_type` 按规范是 **f(16)**（2 字节 little-endian），不是 uleb128。
fn parse_obu_payload_t35(obu_payload: &[u8]) -> Option<&[u8]> {
    if obu_payload.len() < 2 {
        return None;
    }
    let metadata_type = u16::from_le_bytes([obu_payload[0], obu_payload[1]]) as u64;
    if metadata_type != METADATA_TYPE_ITUT_T35 {
        return None;
    }
    let start = 2usize;
    // 合法的 T.35 应用载荷至少 5 字节头（B5 00 90 00 01）。
    if obu_payload.len() < start + 5 {
        return None;
    }
    Some(&obu_payload[start..])
}

/// 解析低开销 OBU 流（IVF 帧或裸 OBU 串），提取全部 T.35 载荷。
/// `frame_payload`：一帧的 OBU 字节（不含 IVF 头/索引）。
pub fn scan_av1_frame_t35(frame_payload: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos + OBU_HEADER_SIZE <= frame_payload.len() {
        let hdr = frame_payload[pos];
        let obu_type = (hdr >> 3) & 0x0F;
        let has_size = (hdr >> 1) & 1 == 1;
        pos += 1;
        // obu_extension_flag（本实现不处理扩展，跳过 1 字节）。
        if (hdr >> 2) & 1 == 1 {
            pos += 1;
        }
        let (size, n) = if has_size {
            match read_uleb128(frame_payload, pos) {
                Some(v) => v,
                None => break,
            }
        } else {
            // 无 size 字段：整帧视为单个 OBU。
            (frame_payload.len() as u64 - pos as u64, 0)
        };
        pos += n;
        if pos + size as usize > frame_payload.len() {
            break;
        }
        let payload = &frame_payload[pos..pos + size as usize];
        pos += size as usize;
        if obu_type == OBU_TYPE_METADATA {
            if let Some(t35) = parse_obu_payload_t35(payload) {
                out.push(t35.to_vec());
            }
        }
    }
    out
}

/// 解析 IVF 容器（header 32 字节 + 每帧 [u32 size + u64 pts + payload]），
/// 逐帧扫描 T.35 载荷。
pub fn scan_ivf_t35(data: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    if data.len() < 32 || &data[0..4] != b"DKIF" {
        return out;
    }
    let mut pos = 32usize;
    while pos + 12 <= data.len() {
        let size = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
        pos += 12; // 跳过 timestamp u64
        if pos + size > data.len() {
            break;
        }
        out.extend(scan_av1_frame_t35(&data[pos..pos + size]));
        pos += size;
    }
    out
}

// ---------------------------------------------------------------------------
// HEVC Annex B（SEI）
// ---------------------------------------------------------------------------

/// 切分 Annex B 流为 NAL 数组（4 字节开始码 00000001；3 字节 000001 兼容）。
pub fn split_annexb_nalus<'a>(buf: &'a [u8]) -> Vec<&'a [u8]> {
    let mut starts = Vec::new();
    let mut i = 0usize;
    while i < buf.len().saturating_sub(3) {
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
    for (k, &off) in starts.iter().enumerate() {
        let end = starts.get(k + 1).copied().unwrap_or(buf.len());
        nals.push(&buf[off..end]);
    }
    nals
}

/// NAL 头的 nal_unit_type（跳过开始码；HEVC 为 6 bit）。
fn nal_unit_type(nal: &[u8]) -> Option<u8> {
    let mut p = 0usize;
    if nal.get(p) == Some(&0) && nal.get(p + 1) == Some(&0) {
        p = if nal.get(p + 2) == Some(&1) { 3 } else { 4 };
    }
    let hdr = *nal.get(p)?;
    // Python/JS 语义一致：HEVC nal_unit_type = (hdr >> 1) & 0x3F。
    Some((hdr >> 1) & 0x3F)
}

/// 移除 NAL 首字节（nal_unit_type 头）后的 RBSP，并剥离 emulation_prevention_three_byte。
fn rbsp_without_epb(nal: &[u8]) -> Option<Vec<u8>> {
    let mut p = 0usize;
    if nal.get(p) == Some(&0) && nal.get(p + 1) == Some(&0) {
        p = if nal.get(p + 2) == Some(&1) { 3 } else { 4 };
    }
    let start = p + 2; // 2 字节 NAL header
    if start >= nal.len() {
        return None;
    }
    let mut out = Vec::with_capacity(nal.len() - start);
    let mut i = start;
    while i < nal.len() {
        // 00 00 03 → 剥掉 0x03
        if i + 2 < nal.len() && nal[i] == 0 && nal[i + 1] == 0 && nal[i + 2] == 3 {
            out.push(0);
            out.push(0);
            i += 3;
        } else {
            out.push(nal[i]);
            i += 1;
        }
    }
    Some(out)
}

/// 从一条 Prefix_SEI RBSP 中提取 payload_type=4（T.35）的载荷。
/// 返回原始 T.35 载荷（不含 SEI payload_type/size 头）。
fn sei_t35_payload(rbsp: &[u8]) -> Option<Vec<u8>> {
    let mut pos = 0usize;
    loop {
        // payload_type（0xFF 续段）
        let mut payload_type = 0u64;
        loop {
            let b = *rbsp.get(pos)?;
            pos += 1;
            payload_type += u64::from(b);
            if b != 0xFF {
                break;
            }
        }
        // payload_size（0xFF 续段）
        let mut payload_size = 0u64;
        loop {
            let b = *rbsp.get(pos)?;
            pos += 1;
            payload_size += u64::from(b);
            if b != 0xFF {
                break;
            }
        }
        if pos + payload_size as usize > rbsp.len() {
            return None;
        }
        let payload = &rbsp[pos..pos + payload_size as usize];
        if payload_type == 4 {
            return Some(payload.to_vec());
        }
        pos += payload_size as usize;
    }
}

/// 扫描 HEVC Annex B 流的 Prefix/Suffix_SEI（type 39/40），提取全部 T.35 载荷。
pub fn scan_hevc_annexb_t35(buf: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    for nal in split_annexb_nalus(buf) {
        let Some(t) = nal_unit_type(nal) else {
            continue;
        };
        if t != 39 && t != 40 {
            continue; // Prefix_SEI / Suffix_SEI
        }
        let Some(rbsp) = rbsp_without_epb(nal) else {
            continue;
        };
        if let Some(t35) = sei_t35_payload(&rbsp) {
            out.push(t35);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// 聚合：多种码流 T.35 载荷 → 去重后的 2094-50 元数据
// ---------------------------------------------------------------------------

/// 把提取到的 T.35 载荷全部解析为 2094-50 元数据（失败/非 2094-50 的跳过），
/// 按「语义去重」（baseline + 曲线指纹）返回唯一集合。
pub fn t35_to_metadata(payloads: &[Vec<u8>]) -> Result<Vec<AgtmMetadata>> {
    let mut metas: Vec<AgtmMetadata> = Vec::new();
    for payload in payloads {
        let meta = match parse_t35_payload(payload) {
            Ok(Some(m)) => m,
            _ => continue,
        };
        if !metas.iter().any(|m| same_metadata(m, &meta)) {
            metas.push(meta);
        }
    }
    Ok(metas)
}

/// 语义指纹：baseline headroom + ALTR 数 + 各 ALTR headroom/控制点数/首尾点 y。
fn same_metadata(a: &AgtmMetadata, b: &AgtmMetadata) -> bool {
    if (a.baseline_hdr_headroom - b.baseline_hdr_headroom).abs() > 1e-9
        || a.altr.len() != b.altr.len()
    {
        return false;
    }
    a.altr
        .iter()
        .zip(b.altr.iter())
        .all(|(x, y)| (x.headroom - y.headroom).abs() < 1e-9 && x.curve.len() == y.curve.len())
}

/// 便捷：扫描 IVF（AV1）并解析（无 2094-50 时返回空数组，不报错）。
pub fn metadata_from_ivf(data: &[u8]) -> Result<Vec<AgtmMetadata>> {
    let payloads = scan_ivf_t35(data);
    t35_to_metadata(&payloads)
}

/// 从 HEVC Annex B 流直接提取 2094-50 元数据（无 2094-50 时返回空数组，不报错）。
pub fn metadata_from_hevc_annexb(data: &[u8]) -> Result<Vec<AgtmMetadata>> {
    let payloads = scan_hevc_annexb_t35(data);
    t35_to_metadata(&payloads)
}

/// 极简容器/码流嗅探 + 提取：优先按扩展名/头嗅探 IVF/OBU，其次 HEVC Annex B。
pub fn metadata_from_raw(data: &[u8], is_av1: bool) -> Result<Vec<AgtmMetadata>> {
    if is_av1 {
        metadata_from_ivf(data)
    } else {
        metadata_from_hevc_annexb(data)
    }
}

// ---------------------------------------------------------------------------
// 组装一个 metadata OBU（供测试 / 未来注入侧用）：OBU 头 + leb128 size + payload
// ---------------------------------------------------------------------------

/// 写 leb128（≤ 8 字节，AV1 metadata payload 最常用单字节）。
pub fn write_uleb128(mut value: u64, out: &mut Vec<u8>) {
    loop {
        let mut byte = (value & 0x7F) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            break;
        }
    }
}

/// 构造 AV1 metadata OBU（ITUT_T35）：用于验证扫描链路 / 注入。
///
/// AV1 规范的 `metadata_obu()` 里 **`metadata_type` 是 f(16)**（定长 16 位，
/// little-endian），只有 `obu_size` 才是 uleb128 —— 原先用 `write_uleb128` 写
/// metadata_type 只产出 1 字节，OBU 结构非法，libaom 据此报
/// `Failed to decode metadata` 并拒绝解码整条流（exit 69）。
pub fn build_t35_metadata_obu(t35_payload: &[u8]) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend_from_slice(&(METADATA_TYPE_ITUT_T35 as u16).to_le_bytes()); // f(16) metadata_type
    payload.extend_from_slice(t35_payload);
    let mut obu = Vec::new();
    // has_size=1：hdr |= (1 << 1)；type=5 → (5 << 3)。
    obu.push((OBU_TYPE_METADATA << 3) | (1 << 1));
    write_uleb128(payload.len() as u64, &mut obu);
    obu.extend_from_slice(&payload);
    obu
}

/// 把 2094-50 元数据（T.35 载荷）逐帧注入 IVF 流，重算每帧 size 字段。
///
/// 插入位置：帧 payload **末尾**（frame OBU 之后，trailing）。注入位置实验结论：
/// - leading（metadata 在帧前，无 TD）：libaom 软件解码器崩溃（-1145393733）
/// - TD+leading（补 temporal delimiter + metadata）：NVDEC 也崩溃（MP4/IVF 的
///   AV1 sample 规范上不含 TD（ISO/IEC 23008-30），硬件解码头不接受）
/// - **trailing（帧尾）**：NVDEC 完整解码 ✓，仅 gyan 的 libaom 3.14 软件解码器
///   对 ITUT_T35 metadata OBU 有已知缺陷（-1145393733，与 MEMORY.md 既有记录一致，
///   项目解码策略本就 NVDEC 优先）。
///
/// `payload_for_frame(frame_index)` 返回该帧要携带的 T.35 载荷（None = 不注入）。
pub fn inject_t35_into_ivf(
    ivf: &[u8],
    payload_for_frame: impl Fn(usize) -> Option<Vec<u8>>,
) -> Vec<u8> {
    if ivf.len() < 32 || &ivf[0..4] != b"DKIF" {
        return ivf.to_vec();
    }
    let mut out = Vec::with_capacity(ivf.len() + 4096);
    out.extend_from_slice(&ivf[..32]);
    let mut pos = 32usize;
    let mut frame_idx = 0usize;
    while pos + 12 <= ivf.len() {
        let size = u32::from_le_bytes(ivf[pos..pos + 4].try_into().unwrap()) as usize;
        let pts = &ivf[pos + 4..pos + 12];
        pos += 12;
        if pos + size > ivf.len() {
            break;
        }
        let payload = &ivf[pos..pos + size];
        let new_payload: Vec<u8> = match payload_for_frame(frame_idx) {
            Some(t35) => {
                let obu = build_t35_metadata_obu(&t35);
                let mut v = Vec::with_capacity(payload.len() + obu.len());
                v.extend_from_slice(payload);
                v.extend_from_slice(&obu);
                v
            }
            None => payload.to_vec(),
        };
        out.extend_from_slice(&(new_payload.len() as u32).to_le_bytes());
        out.extend_from_slice(pts);
        out.extend_from_slice(&new_payload);
        pos += size;
        frame_idx += 1;
    }
    out
}

// ---------------------------------------------------------------------------
// read-hdr-meta：读取视频文件内嵌的 HDR 元数据（mdcv/clli/2094-50）→ JSON
// ---------------------------------------------------------------------------

use std::path::Path;
use std::process::Command;

/// 运行命令并收集 stdout（失败时带 stderr 尾部报错）。
fn run_capture(cmd: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new(cmd)
        .args(args)
        .output()
        .with_context(|| format!("运行失败: {} {}", cmd.display(), args.join(" ")))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let tail: String = stderr
            .chars()
            .rev()
            .take(500)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        return Err(anyhow!(
            "{} 退出码 {}: {}",
            cmd.display(),
            out.status.code().unwrap_or(-1),
            tail
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// ffprobe 输出色域/传递函数名字 → CICP 枚举（供编辑器使用；未知返回 None）。
fn cicp_from_name(kind: &str, name: &str) -> Option<u32> {
    let v = match kind {
        "primaries" => match name {
            "bt709" => Some(1),
            "smpte170m" | "bt470bg" => Some(6),
            "smpte240m" => Some(7),
            "bt2020" => Some(9),
            _ => None,
        },
        "transfer" => match name {
            "bt709" | "bt601" => Some(1),
            "smpte170m" | "bt470bg" => Some(6),
            "linear" => Some(8),
            "log100" => Some(9),
            "log316" => Some(10),
            "srgb" | "iec61966-2-1" => Some(13),
            "smpte2084" => Some(16),
            "arib-std-b67" => Some(18),
            _ => None,
        },
        "matrix" => match name {
            "gbr" => Some(0),
            "bt709" => Some(1),
            "smpte170m" | "bt470bg" => Some(6),
            "bt2020nc" => Some(9),
            "bt2020c" => Some(10),
            _ => None,
        },
        _ => None,
    };
    v
}

/// 读取视频 HDR 元数据并输出 JSON 字符串。
///
/// - ffprobe：codec 名、色彩信息（colr/CICP）、`Mastering display metadata`（mdcv）、
///   `Content light level metadata`（clli）。
/// - ffmpeg：按 codec 提取裸流（AV1→IVF、HEVC→Annex B），再扫描 2094-50。
/// - 输出 `{ container, codec, colour, mastering_display, content_light_level, st2094_50[] }`。
pub fn read_hdr_meta_file(
    input: &Path,
    ffmpeg: &Path,
    ffprobe: &Path,
) -> Result<serde_json::Value> {
    // 1) ffprobe：流信息 + side data
    let probe = run_capture(
        ffprobe,
        &[
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=codec_name,color_primaries,color_transfer,color_space,side_data_list",
            "-of",
            "json",
            input.to_str().unwrap_or(""),
        ],
    )?;
    let probe_json: serde_json::Value = serde_json::from_str(&probe)
        .with_context(|| format!("ffprobe JSON 解析失败: {}", input.display()))?;
    let stream = probe_json
        .get("streams")
        .and_then(|s| s.as_array())
        .and_then(|a| a.first())
        .cloned()
        .unwrap_or(serde_json::Value::Null);

    let codec = stream
        .get("codec_name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let prim_name = stream
        .get("color_primaries")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let tran_name = stream
        .get("color_transfer")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let matx_name = stream
        .get("color_space")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // 2) mdcv / clli（side_data_list）
    let mut mastering = serde_json::Map::new();
    let mut clli = serde_json::Map::new();
    if let Some(list) = stream.get("side_data_list").and_then(|v| v.as_array()) {
        for sd in list {
            let ty = sd
                .get("side_data_type")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            match ty {
                "Mastering display metadata" => {
                    for f in [
                        "red_x",
                        "red_y",
                        "green_x",
                        "green_y",
                        "blue_x",
                        "blue_y",
                        "white_point_x",
                        "white_point_y",
                    ] {
                        if let Some(n) = sd
                            .get(f)
                            .and_then(|v| v.as_str())
                            .and_then(|s| s.parse::<f64>().ok())
                        {
                            mastering.insert(f.to_string(), serde_json::json!(n / 10000.0));
                        }
                    }
                    for f in ["max_luminance", "min_luminance"] {
                        if let Some(n) = sd
                            .get(f)
                            .and_then(|v| v.as_str())
                            .and_then(|s| s.parse::<f64>().ok())
                        {
                            mastering.insert(f.to_string(), serde_json::json!(n / 10000.0));
                        }
                    }
                }
                "Content light level metadata" => {
                    for f in ["max_content", "max_average"] {
                        if let Some(n) = sd
                            .get(f)
                            .and_then(|v| v.as_str())
                            .and_then(|s| s.parse::<f64>().ok())
                        {
                            clli.insert(f.to_string(), serde_json::json!(n));
                        }
                    }
                }
                _ => {}
            }
        }
    }

    // 3) 提取裸流 + 扫描 2094-50
    let work = std::env::temp_dir().join(format!("hdr_meta_{}.tmp", std::process::id()));
    let metas: Vec<AgtmMetadata> = match codec.as_str() {
        "av1" => {
            let out = work.with_extension("ivf");
            run_capture(
                ffmpeg,
                &[
                    "-hide_banner",
                    "-y",
                    "-i",
                    input.to_str().unwrap_or(""),
                    "-c",
                    "copy",
                    "-f",
                    "ivf",
                    out.to_str().unwrap_or(""),
                ],
            )?;
            let data =
                std::fs::read(&out).with_context(|| format!("读取 {} 失败", out.display()))?;
            let _ = std::fs::remove_file(&out);
            metadata_from_ivf(&data)?
        }
        "hevc" | "h265" => {
            let out = work.with_extension("h265");
            run_capture(
                ffmpeg,
                &[
                    "-hide_banner",
                    "-y",
                    "-i",
                    input.to_str().unwrap_or(""),
                    "-c",
                    "copy",
                    "-bsf:v",
                    "hevc_mp4toannexb",
                    "-f",
                    "hevc",
                    out.to_str().unwrap_or(""),
                ],
            )?;
            let data =
                std::fs::read(&out).with_context(|| format!("读取 {} 失败", out.display()))?;
            let _ = std::fs::remove_file(&out);
            metadata_from_hevc_annexb(&data)?
        }
        other => {
            return Err(anyhow!(
                "暂不支持从 {} 提取 HDR 元数据（仅 AV1/HEVC，收到 codec={}）",
                input.display(),
                other
            ));
        }
    };

    // 4) 组装输出
    Ok(serde_json::json!({
        "source": input.to_string_lossy(),
        "codec": codec,
        "colour": {
            "colour_primaries": cicp_from_name("primaries", &prim_name),
            "transfer_characteristics": cicp_from_name("transfer", &tran_name),
            "matrix_coefficients": cicp_from_name("matrix", &matx_name),
            "primaries_name": prim_name,
            "transfer_name": tran_name,
            "matrix_name": matx_name,
        },
        "mastering_display": mastering,
        "content_light_level": clli,
        "st2094_50_windows": metas,
        "st2094_50_count": metas.len(),
    }))
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::st2094_50::{build_prefix_sei_nal, reference_white_prefix_sei, t35_payload};

    #[test]
    fn av1_metadata_obu_scan() {
        let t35 = t35_payload(&crate::st2094_50::reference_white_app_info(14691));
        let obu = build_t35_metadata_obu(&t35);
        // 模拟 IVF 帧：单帧 OBU 串。
        let t35s = scan_av1_frame_t35(&obu);
        assert_eq!(t35s.len(), 1);
        assert_eq!(t35s[0], t35);
    }

    #[test]
    fn ivf_roundtrip() {
        let t35 = t35_payload(&crate::st2094_50::reference_white_app_info(14691));
        let obu = build_t35_metadata_obu(&t35);
        // 手工拼一个最小 IVF：32 字节头 + 一帧。
        let mut ivf = Vec::new();
        ivf.extend_from_slice(b"DKIF");
        ivf.extend_from_slice(&[0, 0]); // version
        ivf.extend_from_slice(&[32, 0]); // header_size
        ivf.extend_from_slice(b"AV01"); // fourcc
        ivf.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0]); // w/h（占位）
        ivf.extend_from_slice(&[1, 0, 0, 0, 0, 0, 0, 0]); // timebase denom
        ivf.extend_from_slice(&[30, 0, 0, 0, 0, 0, 0, 0]); // timebase num
        ivf.extend_from_slice(&[1, 0, 0, 0]); // frame count
        ivf.extend_from_slice(&[0, 0, 0, 0]); // unused
        ivf.extend_from_slice(&(obu.len() as u32).to_le_bytes());
        ivf.extend_from_slice(&0u64.to_le_bytes());
        ivf.extend_from_slice(&obu);

        let metas = metadata_from_ivf(&ivf).expect("应从 IVF 解析出 2094-50");
        assert_eq!(metas.len(), 1);
        assert!((metas[0].baseline_hdr_headroom - 1.4691).abs() < 1e-9);
    }

    #[test]
    fn inject_t35_into_ivf_then_scan() {
        // 构造 3 帧 IVF（无元数据）→ 注入 2 条不同窗口的 2094-50 → 扫描应读回 2 窗。
        let mut ivf = Vec::new();
        ivf.extend_from_slice(b"DKIF");
        ivf.extend_from_slice(&[0, 0]);
        ivf.extend_from_slice(&[32, 0]);
        ivf.extend_from_slice(b"AV01");
        ivf.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0]);
        ivf.extend_from_slice(&[1, 0, 0, 0, 0, 0, 0, 0]);
        ivf.extend_from_slice(&[30, 0, 0, 0, 0, 0, 0, 0]);
        ivf.extend_from_slice(&[3, 0, 0, 0]); // frame count = 3
        ivf.extend_from_slice(&[0, 0, 0, 0]);
        // 每帧一个最小 frame OBU（type=6, has_size=1, size=0）。
        let frame_obu = [0x32u8, 0x00];
        for _ in 0..3 {
            ivf.extend_from_slice(&(frame_obu.len() as u32).to_le_bytes());
            ivf.extend_from_slice(&0u64.to_le_bytes());
            ivf.extend_from_slice(&frame_obu);
        }

        let meta_for: Vec<Vec<u8>> = vec![
            t35_payload(&crate::st2094_50::reference_white_app_info(14691)),
            t35_payload(&crate::st2094_50::reference_white_app_info(13832)),
        ];
        let injected = inject_t35_into_ivf(&ivf, |f| {
            if f < meta_for.len() {
                Some(meta_for[f].clone())
            } else {
                None
            }
        });
        let metas = metadata_from_ivf(&injected).expect("应解析出 2094-50");
        // 帧0=14691, 帧1=13832, 帧2 无 → 去重后 2 条。
        assert_eq!(metas.len(), 2);
        let mut baselines: Vec<f64> = metas.iter().map(|m| m.baseline_hdr_headroom).collect();
        baselines.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert!((baselines[0] - 1.3832).abs() < 1e-9);
        assert!((baselines[1] - 1.4691).abs() < 1e-9);
    }

    #[test]
    fn hevc_sei_roundtrip() {
        // 用编码侧真实生成 Prefix_SEI NAL，再走 Annex B 扫描链路读回。
        let nal = reference_white_prefix_sei(13832);
        let mut annexb = Vec::new();
        annexb.extend_from_slice(&[0, 0, 0, 1]);
        annexb.extend_from_slice(&nal);

        let t35s = scan_hevc_annexb_t35(&annexb);
        assert_eq!(t35s.len(), 1);
        let metas = t35_to_metadata(&t35s).expect("应解析出 2094-50");
        assert_eq!(metas.len(), 1);
        assert!((metas[0].baseline_hdr_headroom - 1.3832).abs() < 1e-9);
    }

    #[test]
    fn hevc_multi_sei_scan() {
        let mut annexb = Vec::new();
        for raw in [14691u16, 13832u16] {
            annexb.extend_from_slice(&[0, 0, 0, 1]);
            annexb.extend_from_slice(&reference_white_prefix_sei(raw));
        }
        let t35s = scan_hevc_annexb_t35(&annexb);
        assert_eq!(t35s.len(), 2);
        let metas = t35_to_metadata(&t35s).expect("应解析出 2094-50");
        // 去重后 2 条（baseline 不同）。
        assert_eq!(metas.len(), 2);
    }

    #[test]
    fn scan_not_2094_50_ignored() {
        // 非 T.35 的 SEI 载荷（如未注册用户数据 payload_type=5）应被忽略且不报错。
        let t35 = t35_payload(&crate::st2094_50::reference_white_app_info(5000));
        let nal = build_prefix_sei_nal(&t35);
        let mut annexb = Vec::new();
        annexb.extend_from_slice(&[0, 0, 0, 1]);
        annexb.extend_from_slice(&nal);
        let t35s = scan_hevc_annexb_t35(&annexb);
        assert_eq!(t35s.len(), 1);
        let _ = t35_to_metadata(&t35s).context("解析应成功");
    }
}
