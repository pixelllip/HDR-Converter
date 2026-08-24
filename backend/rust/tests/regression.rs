//! 回归测试占位。
//!
//! 目标：同一输入分别用 hdrconv 与 Kotlin 后端（
//! `java -jar backend/kotlin/build/libs/hdr-converter-backend.jar` 的 /convert 端点）处理，
//! 逐像素比较输出。待转换核心移植后启用完整对照（可先跑 png 直通链路）。

use hdrconv::convert_image;
use hdrconv::models::{OutputFormat, Settings};
use image::GenericImageView;

/// 生成 4x4 渐变测试图，返回 (输入路径, 输出路径)。
/// label 用于区分并行运行的测试（cargo test 默认并行）；产物放 target/test_tmp。
fn temp_paths(label: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/test_tmp");
    std::fs::create_dir_all(&dir).expect("创建测试目录失败");
    let tag = std::process::id();
    let input = dir.join(format!("hdrconv_{label}_in_{tag}.png"));
    let output = dir.join(format!("hdrconv_{label}_out_{tag}.png"));
    let mut buf = image::RgbaImage::new(4, 4);
    for (x, y, p) in buf.enumerate_pixels_mut() {
        *p = image::Rgba([(x * 60) as u8, (y * 60) as u8, 128, 255]);
    }
    buf.save(&input).expect("写入测试输入失败");
    (input, output)
}

/// 直通管线的 PNG 往返：转换核心尚未移植，验证 IO/封装链路可用。
#[test]
fn passthrough_png_roundtrip() {
    let (input, output) = temp_paths("roundtrip");
    let settings = Settings::default();

    convert_image(&input, &output, &settings, OutputFormat::Png)
        .expect("直接转 PNG 应成功（直通链路）");

    let img = image::open(&output).expect("输出应可解码");
    assert_eq!(img.dimensions(), (4, 4));

    let _ = std::fs::remove_file(input);
    let _ = std::fs::remove_file(output);
}

/// Ultra HDR 端到端结构：增益图 + XMP(GContainer/hdrgm) + MPF + ICC + 次图 JPEG。
#[test]
fn ultra_hdr_basic_structure() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let input = root.join("tests/tmp_uhdr_input.png"); // 640x360
    assert!(input.exists(), "缺少测试输入 tests/tmp_uhdr_input.png");
    let output = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target/test_tmp/uhdr_out.jpg");
    let settings = Settings::default();

    convert_image(&input, &output, &settings, OutputFormat::UltraHdr)
        .expect("ultra-hdr 转换应成功");
    let bytes = std::fs::read(&output).expect("输出应存在");

    // 结构与段落
    assert!(find_bytes(&bytes, b"ICC_PROFILE\0").is_some(), "应含 ICC_PROFILE 段");
    assert!(find_bytes(&bytes, b"MPF\0").is_some(), "应含 MPF 索引段");
    assert!(find_bytes(&bytes, b"hdrgm:GainMapMax=").is_some(), "应含 hdrgm XMP");
    assert!(find_bytes(&bytes, b"Item:Length=").is_some(), "应含 GContainer Item");
    assert!(find_bytes(&bytes, b"Container:Directory").is_some(), "应含 GContainer 目录");

    // 主图像可解码、尺寸正确
    let img = image::load_from_memory(&bytes).expect("主图像应可解码");
    assert_eq!(img.dimensions(), (640, 360));

    // 主图策略：输入未声明 → sRGB 像素转 Display-P3 + P3 ICC → 探测应为 Display-P3
    assert_eq!(
        hdrconv::colorspace::detect(&output),
        hdrconv::colorspace::InputColorSpace::DisplayP3,
        "主图像注入 P3 ICC，探测应识别为 Display-P3"
    );

    let _ = std::fs::remove_file(output);
}

/// 程序化 sRGB ICC：头/标签表结构，标签按签名排序。
#[test]
fn build_srgb_icc_portable() {
    let icc = hdrconv::ultra_hdr::build_srgb_icc();
    assert!(icc.len() > 128);
    assert_eq!(&icc[36..40], b"acsp");
    let size = u32::from_be_bytes([icc[0], icc[1], icc[2], icc[3]]) as usize;
    assert_eq!(size, icc.len(), "ICC size 字段应等于实际长度");
    let n = u32::from_be_bytes([icc[128], icc[129], icc[130], icc[131]]) as usize;
    assert_eq!(n, 9, "应含 9 个标签（与 Kotlin buildSrgbIcc 一致）");
    for (i, expect) in [
        "bTRC", "bXYZ", "cprt", "desc", "gTRC", "gXYZ", "rTRC", "rXYZ", "wtpt",
    ]
    .iter()
    .enumerate()
    {
        assert_eq!(&icc[132 + i * 12..136 + i * 12], expect.as_bytes(), "标签 {i} 应按签名排序");
    }
}

/// 双线性下采样基础行为。
#[test]
fn downscale_bilinear_basic() {
    // 4x4 全 200 → 2x2 全 200
    let out = hdrconv::ultra_hdr::downscale_bilinear(&vec![200u8; 16], 4, 4, 2, 2);
    assert_eq!(out, vec![200u8; 4]);
    // 2x2 → 1x1：ys=2, xs=2 时 fy=fx=0，取左上角（与 Kotlin floor 行为一致）
    let out2 = hdrconv::ultra_hdr::downscale_bilinear(&vec![10, 20, 30, 40], 2, 2, 1, 1);
    assert_eq!(out2, vec![10]);
}

/// 自动估算 HDR 强度：纯黑/纯白边界。
#[test]
fn estimate_hdr_intensity_sanity() {
    let black = hdrconv::ultra_hdr::estimate_hdr_intensity(&vec![0u8; 4], 1, 1);
    assert_eq!(black.hdr_intensity, 0.96, "无高光 → 0.8*0.9=0.72 被下限钳到 0.96");
    assert!((black.max_boost - 2.0f64.powf(0.96)).abs() < 1e-9);

    let white = hdrconv::ultra_hdr::estimate_hdr_intensity(&vec![255u8; 4], 1, 1);
    assert!(white.y_p995 > 0.99);
    assert_eq!(white.hl_ratio, 1.0);
    assert!((white.hdr_intensity - 1.575).abs() < 1e-9, "1.5*1.05=1.575");
}

/// 视频帧重建：PAM 头 + 大端 16-bit 数据。
#[test]
fn reconstruct_pam_structure() {
    let settings = Settings::default();
    let rgba = vec![255u8; 8]; // 2x1 纯白
    let pam = hdrconv::ultra_hdr::reconstruct_linear_hdr_frame(
        &rgba,
        2,
        1,
        &settings,
        8.0,
        settings.ev(),
    )
    .expect("重建失败");
    let header = b"P7\nWIDTH 2\nHEIGHT 1\nDEPTH 3\nMAXVAL 65535\nTUPLTYPE RGB\nENDHDR\n";
    assert!(pam.starts_with(header), "PAM 头不正确");
    assert_eq!(pam.len(), header.len() + 2 * 1 * 3 * 2, "数据长度应为 n*6");
    // 纯白 → 高光增益 >1 → 输出线性值显著高于 SDR 参考（>20000/65535）
    let first = u16::from_be_bytes([pam[header.len()], pam[header.len() + 1]]);
    assert!(first > 20000, "纯白像素经增益后应显著高于 SDR 白，实际 {first}");
}

/// Kotlin 后端基准逐像素对照（png 输出，无损）。
///
/// 前置：生成基准文件
/// ```bash
/// node tests/rust_baseline.js     # 生成 tests/rust_ref_input.png + tests/rust_ref_kotlin.png
/// ```
/// 然后运行：
/// ```bash
/// cargo test -- --ignored
/// ```
///
/// 对照条件：Rust 侧 Settings::default()（peak=574, white=203, gamma=0.9, rgb=0.96,1,1）
/// 与基准脚本传给 Kotlin /convert 的参数完全一致；Kotlin 侧已用 HDR_GPU_DISABLE=1 强制 CPU。
#[test]
#[ignore = "需先运行 node tests/rust_baseline.js 生成 Kotlin 基准"]
fn png_pixels_match_kotlin_baseline() {
    use std::path::Path;

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."); // hdr_electron/（tests/ 在仓库根）
    let input = root.join("tests/rust_ref_input.png");
    let kotlin = root.join("tests/rust_ref_kotlin.png");
    assert!(input.exists(), "缺少基准输入，请先运行: node tests/rust_baseline.js");
    assert!(kotlin.exists(), "缺少 Kotlin 基准输出，请先运行: node tests/rust_baseline.js");

    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/test_tmp/rust_ref_rust.png");
    let mut settings = Settings::default();
    settings.icc_path = Some("../../assets/2020_profile.icc".to_string());
    convert_image(&input, &out, &settings, OutputFormat::Png).expect("Rust 转换失败");

    let a = image::open(&kotlin)
        .expect("解码 Kotlin 输出失败")
        .to_rgba8()
        .into_raw();
    let b = image::open(&out).expect("解码 Rust 输出失败").to_rgba8().into_raw();
    assert_eq!(a.len(), b.len(), "像素数不一致: kotlin={} rust={}", a.len(), b.len());

    let mut ndiff = 0usize;
    let mut maxdiff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        let d = x.abs_diff(*y);
        if d > 0 {
            ndiff += 1;
            maxdiff = maxdiff.max(d);
        }
    }
    assert_eq!(
        ndiff,
        0,
        "与 Kotlin 基准不一致: {ndiff}/{} 像素不同, 最大通道差 {maxdiff}",
        a.len()
    );
}

/// PNG iCCP 注入往返：注入后仍可解码、像素不变、iCCP 内压缩数据解压后等于原 ICC。
#[test]
fn png_iccp_injection_roundtrip() {
    use hdrconv::convert::{encode_png_bytes, ImageData};
    use std::io::Read;

    let icc = std::fs::read("../../assets/2020_profile.icc").expect("读取 ICC 失败");
    let img = ImageData {
        pixels: vec![10, 20, 30, 255, 200, 100, 50, 255],
        width: 2,
        height: 1,
    };
    let plain = encode_png_bytes(&img).expect("PNG 编码失败");
    let injected = hdrconv::icc::inject_icc_into_png(&plain, &icc).expect("iCCP 注入失败");

    // 解码后像素不变
    let decoded = image::load_from_memory(&injected)
        .expect("注入后 PNG 应可解码")
        .to_rgba8();
    assert_eq!(decoded.dimensions(), (2, 1));
    assert_eq!(decoded.into_raw(), img.pixels);

    // 找到 iCCP chunk，解压后 == 原 ICC
    let pos = find_bytes(&injected, b"iCCP")
        .unwrap_or_else(|| panic!("未找到 iCCP chunk"));
    let data_start = pos + 4; // chunk 数据起点（type 之后）
    let len = u32::from_be_bytes([
        injected[pos - 4],
        injected[pos - 3],
        injected[pos - 2],
        injected[pos - 1],
    ]) as usize;
    let data = &injected[data_start..data_start + len];
    // 名称 "BT.2020\0" + 压缩方法 0
    assert_eq!(&data[..8], b"BT.2020\0", "iCCP 名称应硬编码 BT.2020（Kotlin 行为）");
    assert_eq!(data[8], 0, "压缩方法应为 deflate");
    let mut dec = flate2::read::ZlibDecoder::new(&data[9..]);
    let mut out = Vec::new();
    dec.read_to_end(&mut out).expect("解压 iCCP 失败");
    assert_eq!(out, icc, "iCCP 解压数据应等于原 ICC");
}

/// JPEG APP2 注入往返：注入后仍可解码、尺寸不变、含 ICC_PROFILE 签名。
#[test]
fn jpeg_app2_injection_roundtrip() {
    use hdrconv::convert::{encode_jpeg_bytes, ImageData};

    let icc = std::fs::read("../../assets/2020_profile.icc").expect("读取 ICC 失败");
    let img = ImageData {
        pixels: vec![10, 20, 30, 255, 200, 100, 50, 255, 90, 60, 120, 255, 250, 250, 240, 255],
        width: 2,
        height: 2,
    };
    let plain = encode_jpeg_bytes(&img, 0.9).expect("JPEG 编码失败");
    let injected = hdrconv::icc::inject_icc_into_jpeg(&plain, &icc).expect("APP2 注入失败");

    assert!(injected.windows(12).any(|w| w == b"ICC_PROFILE\0"), "应含 ICC_PROFILE 签名");
    assert!(injected.len() > plain.len(), "注入后应变大");

    let decoded = image::load_from_memory(&injected)
        .expect("注入后 JPEG 应可解码");
    assert_eq!(decoded.dimensions(), (2, 2));
}

/// jpg-icc 端到端：Rec.2020/PQ 变换 + APP2 ICC 注入。
#[test]
fn jpg_icc_end_to_end() {
    let (input, output) = temp_paths("jpegicc");
    let mut settings = Settings::default();
    settings.icc_path = Some("../../assets/2020_profile.icc".to_string());

    convert_image(&input, &output, &settings, OutputFormat::JpgIcc)
        .expect("jpg-icc 转换应成功");
    let bytes = std::fs::read(&output).expect("输出应存在");
    assert!(bytes.windows(12).any(|w| w == b"ICC_PROFILE\0"), "输出应含 ICC_PROFILE");

    let _ = std::fs::remove_file(input);
    let _ = std::fs::remove_file(output);
}

/// 在字节流中查找子串位置。
fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// 从 Ultra HDR JPEG 字节中提取 hdrgm 属性数值（如 `hdrgm:GainMapMax="1.5"`）。
fn xmp_num(bytes: &[u8], key: &str) -> Option<f64> {
    let needle = format!("hdrgm:{key}=\"");
    let pos = bytes.windows(needle.len()).position(|w| w == needle.as_bytes())?;
    let start = pos + needle.len();
    let rel_end = bytes[start..].iter().position(|&b| b == b'"')?;
    std::str::from_utf8(&bytes[start..start + rel_end])
        .ok()?
        .parse::<f64>()
        .ok()
}

/// 与 Kotlin 的 Ultra HDR **XMP 数值**对照：增益图统计量（min/max boost 等）
/// 在 JPEG 编码前算出，两个实现应完全一致（JPEG 编码器不同，字节流本身不可比）。
#[test]
#[ignore = "需先运行 node tests/rust_baseline.js 生成 Kotlin 基准（含 ultra-hdr 输出）"]
fn ultra_hdr_xmp_matches_kotlin_baseline() {
    use std::path::Path;

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let input = root.join("tests/rust_ref_input.png");
    let kotlin = root.join("tests/rust_ref_kotlin_uhdr.jpg");
    assert!(input.exists(), "缺少基准输入，请先运行: node tests/rust_baseline.js");
    assert!(kotlin.exists(), "缺少 Kotlin ultra-hdr 基准，请先运行: node tests/rust_baseline.js");

    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/test_tmp/rust_ref_rust_uhdr.jpg");
    let settings = Settings::default();
    convert_image(&input, &out, &settings, OutputFormat::UltraHdr)
        .expect("Rust ultra-hdr 转换失败");

    let kotlin_bytes = std::fs::read(&kotlin).expect("读取 Kotlin 基准失败");
    let rust_bytes = std::fs::read(&out).expect("读取 Rust 输出失败");

    for key in [
        "GainMapMin",
        "GainMapMax",
        "Gamma",
        "OffsetSDR",
        "OffsetHDR",
        "HDRCapacityMin",
        "HDRCapacityMax",
    ] {
        let vk = xmp_num(&kotlin_bytes, key).unwrap_or_else(|| panic!("Kotlin 基准缺少 hdrgm:{key}"));
        let vr = xmp_num(&rust_bytes, key).unwrap_or_else(|| panic!("Rust 输出缺少 hdrgm:{key}"));
        assert!(
            (vk - vr).abs() < 1e-9,
            "hdrgm:{key} 与 Kotlin 不一致: Kotlin={vk}, Rust={vr}"
        );
    }
    let _ = std::fs::remove_file(out);
}

// ---------------------------------------------------------------
// 色彩空间探测（← ColorSpaceDetector.kt）
// ---------------------------------------------------------------

/// 基准输入 PNG（仅 IHDR/IDAT/IEND）应探测为 UNKNOWN——
/// 与 Kotlin 基准打印的「未声明（按 sRGB 假设）」一致。
#[test]
fn detect_baseline_png_is_unknown() {
    let input = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/rust_ref_input.png");
    if !input.exists() {
        return; // 未生成基准时跳过
    }
    assert_eq!(
        hdrconv::colorspace::detect(&input),
        hdrconv::colorspace::InputColorSpace::Unknown
    );
}

/// 含 sRGB chunk 的 PNG → Srgb。
#[test]
fn detect_png_srgb_chunk() {
    let mut png = Vec::new();
    png.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
    // sRGB chunk：len=0 + "sRGB" + crc
    png.extend_from_slice(&0u32.to_be_bytes());
    png.extend_from_slice(b"sRGB");
    png.extend_from_slice(&hdrconv::icc::crc32(b"sRGB").to_be_bytes());
    let bytes = png;
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/test_tmp/srgb_chunk.png");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    assert_eq!(
        hdrconv::colorspace::detect(&path),
        hdrconv::colorspace::InputColorSpace::Srgb
    );
    let _ = std::fs::remove_file(path);
}

/// JPEG + APP1 EXIF（TIFF II，ColorSpace tag=1）→ Srgb。
#[test]
fn detect_jpeg_exif_srgb() {
    // "Exif\0\0" + II TIFF：IFD0 偏移 8，1 个 tag 0xA001 值 1（sRGB）
    let mut exif = Vec::new();
    exif.extend_from_slice(b"Exif\0\0");
    exif.extend_from_slice(b"II");
    exif.push(0x2A);
    exif.push(0x00);
    exif.extend_from_slice(&8u32.to_le_bytes()); // IFD0 offset
    exif.extend_from_slice(&1u16.to_le_bytes()); // nTags
    exif.extend_from_slice(&0xA001u16.to_le_bytes()); // tag ColorSpace
    exif.extend_from_slice(&3u16.to_le_bytes()); // type SHORT
    exif.extend_from_slice(&1u32.to_le_bytes()); // count
    exif.extend_from_slice(&1u32.to_le_bytes()); // value = 1 (sRGB)

    let mut jpg = Vec::new();
    jpg.extend_from_slice(&[0xFF, 0xD8]); // SOI
    jpg.extend_from_slice(&[0xFF, 0xE1]); // APP1
    jpg.extend_from_slice(&((2 + exif.len()) as u16).to_be_bytes());
    jpg.extend_from_slice(&exif);
    jpg.extend_from_slice(&[0xFF, 0xD9]); // EOI

    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/test_tmp/exif_srgb.jpg");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, &jpg).unwrap();
    assert_eq!(
        hdrconv::colorspace::detect(&path),
        hdrconv::colorspace::InputColorSpace::Srgb
    );
    let _ = std::fs::remove_file(path);
}

/// JPEG + APP2 注入 Display-P3 ICC → 主色匹配 → DisplayP3。
#[test]
fn detect_jpeg_icc_display_p3() {
    use hdrconv::convert::{encode_jpeg_bytes, ImageData};

    let icc_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/display_p3_primary.icc");
    if !icc_path.exists() {
        return; // 无该资产时跳过
    }
    let icc = std::fs::read(&icc_path).unwrap();
    let img = ImageData {
        pixels: vec![10, 20, 30, 255, 200, 100, 50, 255],
        width: 2,
        height: 1,
    };
    let plain = encode_jpeg_bytes(&img, 0.9).unwrap();
    let injected = hdrconv::icc::inject_icc_into_jpeg(&plain, &icc).unwrap();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/test_tmp/p3_icc.jpg");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, &injected).unwrap();
    assert_eq!(
        hdrconv::colorspace::detect(&path),
        hdrconv::colorspace::InputColorSpace::DisplayP3,
        "注入 Display-P3 ICC 的 JPEG 应识别为 Display-P3"
    );
    let _ = std::fs::remove_file(path);
}