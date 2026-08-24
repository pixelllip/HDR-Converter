//! ← IccInjector.kt：ICC 配置文件注入。
//!
//! 移植对照（backend/kotlin/src/main/kotlin/com/hdrconverter/IccInjector.kt）：
//! - `injectIccIntoPng` (行 28) → `inject_icc_into_png`：
//!   解析 PNG chunk，在第一个 IDAT 前插入 iCCP（数据 = "BT.2020\0" + 压缩方法0 + zlib(ICC)）
//! - `injectIccIntoJpeg` (行 89) → `inject_icc_into_jpeg`：
//!   扫描 SOI 后的前置 APP/COM 段，在其后插入单个 APP2 'ICC_PROFILE\0' 段（seq=1,total=1，不分段）
//! - `readIccProfile` (行 128) → `read_icc_profile`
//!
//! 与 Kotlin 逐位一致：iCCP profile 名硬编码 "BT.2020"（Kotlin 行为），
//! CRC-32 多项式 0xEDB88320、zlib 格式（含头部 + Adler-32，Deflater.DEFAULT_COMPRESSION=6
//! → flate2 Compression::default()）。

use std::io::Write;
use std::path::Path;

use anyhow::{bail, Context, Result};

/// 注入 iCCP 块到 PNG 字节流（← `IccInjector.injectIccIntoPng`）。
pub fn inject_icc_into_png(png: &[u8], icc: &[u8]) -> Result<Vec<u8>> {
    if png.len() < 8 {
        bail!("PNG 数据过短（缺签名）");
    }
    let signature = &png[..8];
    let mut chunks: Vec<(String, Vec<u8>)> = Vec::new();
    let mut offset = 8usize;
    while offset + 8 <= png.len() {
        let len =
            u32::from_be_bytes([png[offset], png[offset + 1], png[offset + 2], png[offset + 3]])
                as usize;
        if offset + 8 + len > png.len() {
            bail!("PNG chunk 长度越界");
        }
        let typ = String::from_utf8_lossy(&png[offset + 4..offset + 8]).into_owned();
        let data = png[offset + 8..offset + 8 + len].to_vec();
        chunks.push((typ, data));
        offset += 12 + len;
    }

    // iCCP chunk 数据：名称 "BT.2020\0" + 压缩方法(0=deflate) + zlib(icc)
    let mut compressed = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    compressed.write_all(icc)?;
    let compressed = compressed.finish()?;
    let mut icc_data = Vec::with_capacity(8 + 1 + compressed.len());
    icc_data.extend_from_slice(b"BT.2020\0");
    icc_data.push(0); // compression method: deflate
    icc_data.extend_from_slice(&compressed);

    let idat_index = chunks.iter().position(|(t, _)| t == "IDAT");
    let insert_index = idat_index.unwrap_or(chunks.len());
    let icc_chunk_size = icc_data.len();
    chunks.insert(insert_index, ("iCCP".to_string(), icc_data));

    // 重建 PNG：签名 + 全部 chunk（长度/类型/数据/CRC）
    let mut out = Vec::with_capacity(png.len() + icc_chunk_size + 12);
    out.extend_from_slice(signature);
    for (typ, data) in &chunks {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(typ.as_bytes());
        out.extend_from_slice(data);
        let mut crc_input = Vec::with_capacity(4 + data.len());
        crc_input.extend_from_slice(typ.as_bytes());
        crc_input.extend_from_slice(data);
        out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
    }
    Ok(out)
}

/// 注入 APP2 'ICC_PROFILE' 段到 JPEG 字节流（← `IccInjector.injectIccIntoJpeg`）。
///
/// 位置约定：SOI 之后、所有前置 APP/COM 段之后（即 DQT/SOF/SOS 之前），
/// 标准且各查看器都接受；切勿插到 SOS 之后的熵数据里。
/// Kotlin 行为：**单个** APP2 段（seq=1, total=1），ICC 超过 255 字节也不分段。
pub fn inject_icc_into_jpeg(jpeg: &[u8], icc: &[u8]) -> Result<Vec<u8>> {
    if jpeg.len() < 2 || jpeg[0] != 0xFF || jpeg[1] != 0xD8 {
        bail!("非 JPEG（缺少 SOI 标记）");
    }

    // 构建 APP2 段：FFE2 + segLen(2, 含自身) + "ICC_PROFILE\0"(12) + seq(1) + total(1) + icc
    let sig = b"ICC_PROFILE\0";
    let seg_len = 2 + sig.len() + 2 + icc.len();
    let mut app2 = Vec::with_capacity(2 + seg_len);
    app2.extend_from_slice(&0xFFE2u16.to_be_bytes());
    app2.extend_from_slice(&(seg_len as u16).to_be_bytes());
    app2.extend_from_slice(sig);
    app2.push(1); // sequence number
    app2.push(1); // total number of segments
    app2.extend_from_slice(icc);

    // 找到前置 APP/COM 段结束位置
    let mut insert_pos = 2usize; // SOI 之后
    while insert_pos + 4 <= jpeg.len() {
        let marker = u16::from_be_bytes([jpeg[insert_pos], jpeg[insert_pos + 1]]);
        let is_app = (0xFFE0..=0xFFEF).contains(&marker);
        let is_com = marker == 0xFFFE;
        if !is_app && !is_com {
            break;
        }
        let seg_len_m =
            u16::from_be_bytes([jpeg[insert_pos + 2], jpeg[insert_pos + 3]]) as usize;
        if insert_pos + 2 + seg_len_m > jpeg.len() {
            bail!("JPEG 段长度越界");
        }
        insert_pos += 2 + seg_len_m;
    }

    let mut out = Vec::with_capacity(jpeg.len() + app2.len());
    out.extend_from_slice(&jpeg[..insert_pos]);
    out.extend_from_slice(&app2);
    out.extend_from_slice(&jpeg[insert_pos..]);
    Ok(out)
}

/// 读取 .icc 配置文件（← `IccInjector.readIccProfile`）。
pub fn read_icc_profile(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path).with_context(|| format!("读取 ICC 失败: {}", path.display()))
}

/// 自动探测 2020_profile.icc（对齐 Kotlin `resolveIccProfilePath` 的候选顺序思路）。
pub fn find_default_icc() -> Option<std::path::PathBuf> {
    [
        "assets/2020_profile.icc",
        "../assets/2020_profile.icc",
        "../../assets/2020_profile.icc",
        "../../../assets/2020_profile.icc",
        "backend/kotlin/2020_profile.icc",
    ]
    .iter()
    .map(std::path::PathBuf::from)
    .find(|p| p.exists())
}

/// CRC-32（PNG 标准，多项式 0xEDB88320；← IccInjector.crc32，行为一致）。
pub fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (n, slot) in table.iter_mut().enumerate() {
        let mut c = n as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB88320 ^ (c >> 1) } else { c >> 1 };
        }
        *slot = c;
    }
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc = table[((crc ^ b as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    crc ^ 0xFFFF_FFFF
}