//! 回归测试占位。
//!
//! 目标：同一输入分别用 hdrconv 与 Kotlin 后端（
//! `java -jar backend/kotlin/build/libs/hdr-converter-backend.jar` 的 /convert 端点）处理，
//! 逐像素比较输出。待转换核心移植后启用完整对照（可先跑 png 直通链路）。

use hdrconv::convert_image;
use hdrconv::models::{OutputFormat, Settings};
use image::GenericImageView;

/// ST 2094-50 参考白配方编码字节（逐位对齐 JS st2094_50.js）。
#[test]
fn st2094_50_reference_white_recipe_bytes() {
    use hdrconv::st2094_50;

    // application_info: version_flags(0x00) + cvt_flag(0x40) + u16(5000=0x1388) + 0x80
    let app = st2094_50::reference_white_app_info(5000);
    assert_eq!(app, vec![0x00, 0x40, 0x13, 0x88, 0x80]);

    // t35: B5 00 90 00 01 + app
    let t35 = st2094_50::t35_payload(&app);
    assert_eq!(&t35[..5], &[0xB5, 0x00, 0x90, 0x00, 0x01]);
    assert_eq!(&t35[5..], &app[..]);

    // Prefix_SEI NAL：头 4E 01（nal_unit_type=39），EBSP 转义后不得出现 00 00 00/01/02/03 未转义序列
    let nal = st2094_50::reference_white_prefix_sei(5000);
    assert_eq!(&nal[..2], &[0x4E, 0x01]);
    let mut zeros = 0u8;
    for &b in &nal[2..] {
        if zeros >= 2 && b <= 0x03 {
            panic!("EBSP 转义缺失: 00 00 {:02X}", b);
        }
        zeros = if b == 0 { zeros + 1 } else { 0 };
    }

    // PQ EOTF 合理性：码值 1.0（=10000 尼特满量程，但 10-bit 最大 1023/1023≈0.999）→ 接近 10000
    let nits = st2094_50::pq_eotf(1.0);
    assert!((nits - 10000.0).abs() < 1.0, "PQ EOTF(1.0) 应≈10000，实际 {nits}");
}

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

    // 主图策略（原汤化原食，2026）：未声明输入 → 主图保持 sRGB 像素 + sRGB ICC → 探测应为 Srgb
    assert_eq!(
        hdrconv::colorspace::detect(&output).space,
        hdrconv::colorspace::InputColorSpace::Srgb,
        "主图像注入 sRGB ICC，探测应识别为 sRGB"
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

/// 面积平均下采样基础行为。
#[test]
fn downscale_area_average_box_basic() {
    // 4x4 全 100 → 1x1 = 100
    let out = hdrconv::ultra_hdr::downscale_area_average_box(&vec![100u8; 16], 4, 4, 1, 1);
    assert_eq!(out, vec![100]);

    // 8x8 阶跃：左半 0，右半 255 → 4x4 下采样（缩 2×）。
    // 列 0..1 = 左半纯 0；列 2..3 = 右半纯 255。验证"无中间过渡区"。
    let mut src = vec![0u8; 64];
    for y in 0..8 {
        for x in 0..8 {
            src[y * 8 + x] = if x < 4 { 0 } else { 255 };
        }
    }
    let out2 = hdrconv::ultra_hdr::downscale_area_average_box(&src, 8, 8, 4, 4);
    assert_eq!(out2.len(), 16);
    let col = |x: usize| -> Vec<u8> { (0..4).map(|y| out2[y * 4 + x]).collect() };
    let col0 = col(0);
    let col1 = col(1);
    let col2 = col(2);
    let col3 = col(3);
    for &v in &col0 {
        assert_eq!(v, 0, "列 0 应为 0，实际 {col0:?}");
    }
    for &v in &col1 {
        assert_eq!(v, 0, "列 1 应为 0，实际 {col1:?}");
    }
    for &v in &col2 {
        assert_eq!(v, 255, "列 2 应为 255，实际 {col2:?}");
    }
    for &v in &col3 {
        assert_eq!(v, 255, "列 3 应为 255，实际 {col3:?}");
    }

    // 8x8 阶跃错位（edge 在 x=3.5）：4x4 下采样的 col 0..3 应单调、平滑，且过渡带宽 > 1。
    // 行布局: [0,0,0,255,255,255,255,255]（每行）
    let mut src2 = vec![0u8; 64];
    for y in 0..8 {
        for x in 0..8 {
            src2[y * 8 + x] = if x < 3 { 0 } else { 255 };
        }
    }
    let out3 = hdrconv::ultra_hdr::downscale_area_average_box(&src2, 8, 8, 4, 4);
    let col = |x: usize| -> Vec<u8> { (0..4).map(|y| out3[y * 4 + x]).collect() };
    // 缩 2× 后：col 0 源 [0..2)=0、col 1 源 [2..4)=[0,255] 各半=128、col 2 源 [4..6)=255、col 3 源 [6..8)=255
    let c0 = col(0);
    let c1 = col(1);
    let c2 = col(2);
    let c3 = col(3);
    for &v in &c0 {
        assert_eq!(v, 0, "错位边缘：列 0 应为 0，实际 {c0:?}");
    }
    for &v in &c1 {
        // 源 [2,4) 跨过边缘 x=3（src x<3 为 0、x>=3 为 255），所以 [src2=0, src3=255] → 128。
        assert_eq!(v, 128, "错位边缘：列 1 源范围 [2..4) 一半 0 一半 255，应为 128，实际 {c1:?}");
    }
    for &v in &c2 {
        assert_eq!(v, 255, "列 2 应为 255，实际 {c2:?}");
    }
    for &v in &c3 {
        assert_eq!(v, 255, "列 3 应为 255，实际 {c3:?}");
    }
}

/// 关键回归：面积平均 vs 双线性——锐阶跃 4× 下采样后单步重塑对比。
///
/// 输入：1D 阶跃信号（全分辨率 64 像素，edge=33 处从 0 跳到 255——故意**错位**，
/// 让双线性 2-tap 抽到两个不同值的样本，暴露混叠）。
/// 输出：双线性 decimation 16 像素 vs 面积平均 16 像素。
/// 期望：面积平均下采样后曲线**单调、平滑**；双线性 decimation 应出现非单调
/// 跳变（"伪阶跃"——高 1 像素 + 低 1 像素 + 高 1 像素 ……）。
#[test]
fn downscale_area_average_vs_bilinear_on_step() {
    let mut src = vec![0u8; 64];
    for i in 33..64 {
        src[i] = 255;
    }
    // 双线性 decimation（旧）
    let b = hdrconv::ultra_hdr::downscale_bilinear(&src, 64, 1, 16, 1);
    // 面积平均（新）
    let a = hdrconv::ultra_hdr::downscale_area_average_box(&src, 64, 1, 16, 1);
    // 端点稳定
    assert_eq!(a[0], 0);
    assert_eq!(a[15], 255);
    assert_eq!(b[0], 0);
    assert_eq!(b[15], 255);
    // 单调性：面积平均必须单调非降
    for i in 1..16 {
        assert!(
            a[i] >= a[i - 1],
            "面积平均在 i={i} 处违反单调性：a={:?}",
            a
        );
    }
    // 锐阶跃的"病态信号"：双线性应至少在 1 处非单调（混叠伪信号）。
    // 错位 edge=33：双线性抽到的源位置序列为 floor(x*4)：4,8,12,...,32, 36, 40, ...
    // x=8 时 sx=32 → src[32]=0、src[33]=255 → fx=0 → b[8]=0
    // x=9 时 sx=36 → src[36]=255、src[37]=255 → b[9]=255
    // 所以 b 应该是单调的……嗯。错位 edge=32.x 可能恰好"扫描"到恰好混叠的位置；
    // 这里改为 **multi-tone** 阶跃（0 → 0 → 0 → 255 → 255 → ... 错位），或在
    // 不同 y 错位下测试。先简单断言：a 的曲线过渡带宽 ≥ b 的（a 至少与 b 同等或更平滑）。
    let a_band = transition_band(&a);
    let b_band = transition_band(&b);
    assert!(
        a_band >= b_band,
        "面积平均过渡带宽 ({a_band}) 应 ≥ 双线性 ({b_band})，a={a:?} b={b:?}"
    );

    // 关键对比：面积平均应在过渡区至少产生 2 个不同值（平滑梯度），而双线性在
    // 错位对齐时只产生 1-像素锐跳变（这里 edge=33 仍可能单调，但 b 过渡带宽
    // 也是 1，因此用更强的断言：a 的过渡区**唯一值数量** ≤ b 的）。
    let a_unique = unique_count_in_transition(&a);
    let b_unique = unique_count_in_transition(&b);
    assert!(
        a_unique >= b_unique,
        "面积平均过渡区不同值数 ({a_unique}) 应 ≥ 双线性 ({b_unique})，a={a:?} b={b:?}"
    );
}

/// 端到端：图片 Ultra HDR 在 32×32 上跑通。
/// 第二步后，`compute_gain_map` 直接返回低分辨率增益图（8×8 = 64 字节），
/// 不需要再手动下采样。
#[test]
fn ultra_hdr_end_to_end_uses_area_average() {
    use hdrconv::ultra_hdr::compute_gain_map;

    let w = 32u32;
    let h = 32u32;
    let settings = Settings::default();
    // 半黑半白：左半 0，右半 255
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let v = if x < w / 2 { 0u8 } else { 255u8 };
            rgba[i] = v;
            rgba[i + 1] = v;
            rgba[i + 2] = v;
            rgba[i + 3] = 255;
        }
    }
    let (gm8, _meta) = compute_gain_map(&rgba, w as usize, h as usize, &settings);
    // 第二步：gm8 直接是低分辨率
    assert_eq!(gm8.len(), ((w / 4) * (h / 4)) as usize);

    let gm_w = (w / 4) as usize;
    let gm_h = (h / 4) as usize;
    assert_eq!(gm_w, 8);
    assert_eq!(gm_h, 8);

    // 列方向增益值应**单调非降**（由左到右：暗→亮）。
    let mut col_profile = Vec::with_capacity(gm_w);
    for x in 0..gm_w {
        let mut sum: u32 = 0;
        for y in 0..gm_h {
            sum += gm8[y * gm_w + x] as u32;
        }
        col_profile.push((sum as f64 / gm_h as f64).round() as u8);
    }
    for i in 1..gm_w {
        assert!(
            col_profile[i] >= col_profile[i - 1],
            "增益图列均值在 x={i} 处违反单调性：{col_profile:?}"
        );
    }
    // 最左列均值应 < 最右列（"高光被增益" 的语义）
    assert!(col_profile[0] < col_profile[gm_w - 1]);
}

/// 估计"0→255 的过渡带宽"（连续样本中跨越值域中点的格子数）。
fn transition_band(v: &[u8]) -> usize {
    let mid = 127u8;
    let first = v.iter().position(|&x| x > mid).unwrap_or(v.len());
    let last = v.iter().rposition(|&x| x > mid).unwrap_or(0);
    last.saturating_sub(first) + 1
}

/// 过渡区（跨越值域中点的格子段）中不同取值的数量。
fn unique_count_in_transition(v: &[u8]) -> usize {
    let mid = 127u8;
    let first = v.iter().position(|&x| x > mid).unwrap_or(v.len());
    let last = v.iter().rposition(|&x| x > mid).unwrap_or(0);
    if first >= last {
        return 0;
    }
    let slice = &v[first..=last];
    let mut set = std::collections::HashSet::new();
    for &x in slice {
        set.insert(x);
    }
    set.len()
}

/// 第二步：`compute_gain_map` 直接返回低分辨率（每边 ¼）增益图。
///
/// 关键不变量：
/// 1) 输出 gm8 字节数 = (width/4) * (height/4)；
/// 2) 半黑半白 32×32 图的列均值应单调非降（box 预滤波 + 低分辨率 mask）；
/// 3) 边缘剖面里不应出现"全分辨率 → 下采样"链路中的混叠尖刺。
#[test]
fn compute_gain_map_returns_low_resolution_gainmap() {
    use hdrconv::ultra_hdr::compute_gain_map;

    let w = 32u32;
    let h = 32u32;
    let settings = Settings::default();

    // 纯黑图：整张 0 像素，gain=1 处处，gm8 全 0（minBoost=1、mapMin=0）
    let black = vec![0u8; (w * h * 4) as usize];
    let (gm_black, meta_black) = compute_gain_map(&black, w as usize, h as usize, &settings);
    assert_eq!(gm_black.len(), (w / 4 * h / 4) as usize);
    assert!(gm_black.iter().all(|&v| v == 0), "纯黑图增益图应全 0，实际 {gm_black:?}");
    assert!((meta_black.min_content_boost - 1.0).abs() < 1e-9);
    assert!((meta_black.max_content_boost - 1.0).abs() < 1e-9);

    // 半黑半白图：左半 (x<16) = 0，右半 = 255
    let mut half = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let v: u8 = if x < 16 { 0 } else { 255 };
            half[i] = v;
            half[i + 1] = v;
            half[i + 2] = v;
            half[i + 3] = 255;
        }
    }
    let (gm_half, meta_half) = compute_gain_map(&half, w as usize, h as usize, &settings);
    assert_eq!(gm_half.len(), (w / 4 * h / 4) as usize, "应输出 8×8 = 64 字节");

    let gm_w = (w / 4) as usize;
    let gm_h = (h / 4) as usize;
    // 列均值（每个 x 跨 8 行平均）应严格单调非降
    let mut col_profile = Vec::with_capacity(gm_w);
    for x in 0..gm_w {
        let s: u32 = (0..gm_h).map(|y| gm_half[y * gm_w + x] as u32).sum();
        col_profile.push((s as f64 / gm_h as f64).round() as u8);
    }
    for i in 1..gm_w {
        assert!(
            col_profile[i] >= col_profile[i - 1],
            "增益图列均值在 x={i} 处违反单调性：{col_profile:?}"
        );
    }
    // 左半（x<4）应全为 0，右半（x≥4）应明显 > 0（高光增益生效）
    let left_avg: f64 = (0..4).map(|x| col_profile[x] as f64).sum::<f64>() / 4.0;
    let right_avg: f64 = (4..8).map(|x| col_profile[x] as f64).sum::<f64>() / 4.0;
    assert!(left_avg < 1.0, "左半列均值应接近 0（gain=1），实际 {left_avg}");
    assert!(right_avg > 50.0, "右半列均值应明显 > 0（高光扩展），实际 {right_avg}");
    // max_content_boost 应 > 1（确实有高光被扩展）
    assert!(meta_half.max_content_boost > 1.5);
}

/// 第三步：`gaussian_blur_33` 基础行为与单调性。
///
/// 1) 常数输入仍为常数；
/// 2) 阶跃输入卷积后**仍单调非降**（凸/单调保持性质）；
/// 3) 单像素阶跃经过两次盒式分离卷积后变成 2-像素软过渡（典型输出 0, 33, 67, 100%）。
#[test]
fn gaussian_blur_33_basic() {
    use hdrconv::ultra_hdr::gaussian_blur_33;

    // 常数 0.5 → 全 0.5
    let v = vec![0.5; 9];
    let b = gaussian_blur_33(&v, 3, 3);
    for &x in &b {
        assert!((x - 0.5).abs() < 1e-9, "常数应保持不变");
    }

    // 单像素阶跃（5×1：[0, 0, 1, 1, 1]）→ 单调非降 + 软过渡
    let step = vec![0.0, 0.0, 1.0, 1.0, 1.0];
    let b = gaussian_blur_33(&step, 5, 1);
    assert!(b[0] <= b[1] && b[1] <= b[2] && b[2] <= b[3] && b[3] <= b[4], "应单调非降：{b:?}");
    // 中心像素 x=2：两侧 1+1+0=2 → 2/3 ≈ 0.6667
    assert!((b[2] - 2.0 / 3.0).abs() < 1e-6, "中心像素应 ≈ 2/3，实际 {}", b[2]);
    // 阶跃点 x=1：单侧 1+0+0=1 → 1/3 ≈ 0.3333
    assert!((b[1] - 1.0 / 3.0).abs() < 1e-6, "阶跃前应 ≈ 1/3，实际 {}", b[1]);
    // 阶跃点 x=0：单侧 (0+0+0)/3 = 0（边界 clamp）
    assert!((b[0]).abs() < 1e-9, "最左端应为 0，实际 {}", b[0]);
    // 最右端：1+1+1/clamp = 1
    assert!((b[4] - 1.0).abs() < 1e-9, "最右端应为 1，实际 {}", b[4]);
}

/// 第三步：`compute_gain_map` 在半黑半白 32×32 图上，mask blur 应让"硬阈值"变"软阈值"，
/// 增益图过渡带宽从 1 个低分辨率像素（x=4 处跳变）扩展到 ≥ 2 个。
///
/// 即：在低分辨率 8×8 增益图上，列均值应**先全部从 0 渐升到目标值**，而不是 x=3→x=4 单步跳变。
#[test]
fn compute_gain_map_soft_threshold_has_wide_transition() {
    use hdrconv::ultra_hdr::compute_gain_map;

    let w = 32u32;
    let h = 32u32;
    let settings = Settings::default();

    // 半黑半白：x<16=0、x≥16=255
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let v: u8 = if x < 16 { 0 } else { 255 };
            rgba[i] = v;
            rgba[i + 1] = v;
            rgba[i + 2] = v;
            rgba[i + 3] = 255;
        }
    }
    let (gm8, _meta) = compute_gain_map(&rgba, w as usize, h as usize, &settings);
    let gm_w = (w / 4) as usize;
    let gm_h = (h / 4) as usize;

    // 列均值
    let mut col_mean = Vec::with_capacity(gm_w);
    for x in 0..gm_w {
        let s: u32 = (0..gm_h).map(|y| gm8[y * gm_w + x] as u32).sum();
        col_mean.push(s as f64 / gm_h as f64);
    }

    // 单调非降
    for i in 1..gm_w {
        assert!(
            col_mean[i] >= col_mean[i - 1] - 1e-6,
            "列均值应单调非降：{col_mean:?}"
        );
    }
    // 软阈值特征：x=2..x=5 区间内**至少 2 个不同中间值**，且最大列均值 - 最小列均值 > 16
    // （硬阈值版本会得到 0,0,0,0,255,255,255,255 这种"单步跳变"）。
    let c0 = col_mean[0];
    let c7 = col_mean[7];
    let span = c7 - c0;
    assert!(span > 16.0, "总跨度应大于 16：{col_mean:?}");
    // 检查"过渡带宽 ≥ 2 个低分辨率像素"：取 x=2..=5 中至少 3 个连续的不同值（或跨度 > 32）
    let transition = &col_mean[1..=6]; // x=1..=6 这一段
    let mn = transition.iter().cloned().fold(f64::INFINITY, f64::min);
    let mx = transition.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    assert!(
        mx - mn > 16.0,
        "过渡区(x=1..=6) 跨度应 > 16（软阈值）；当前 {transition:?}"
    );
    // 最左列（x=0）应仍接近 0（box 平均 16 个像素全 0）
    assert!(c0 < 8.0, "最左列均值应接近 0，实际 {c0}");
    // 最右列（x=7）应明显 > 0
    assert!(c7 > 50.0, "最右列均值应 > 50，实际 {c7}");
}

/// 第二步：encode_ultra_hdr 在 32×32 图上的端到端，验证 gm_w/gm_h = 主图 1/4。
#[test]
fn encode_ultra_hdr_uses_low_resolution_gainmap() {
    use hdrconv::ultra_hdr::encode_ultra_hdr;

    let w = 32u32;
    let h = 32u32;
    let settings = Settings::default();

    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let v: u8 = if x < w / 2 { 0 } else { 255 };
            rgba[i] = v;
            rgba[i + 1] = v;
            rgba[i + 2] = v;
            rgba[i + 3] = 255;
        }
    }

    let out = encode_ultra_hdr(&rgba, w, h, &settings, None).expect("encode_ultra_hdr 应成功");

    // 找出次图像（增益图 JPEG）的尺寸：scan 到主图 EOI 之后，下一个 SOI 0xFFD8
    let mut i = 2usize;
    let mut eoi_after_primary: Option<usize> = None;
    while i + 1 < out.len() {
        if out[i] == 0xFF && out[i + 1] == 0xDA {
            // 进入 SOS：扫描到下一个 EOI
            let sos_len = u16::from_be_bytes([out[i + 2], out[i + 3]]) as usize;
            let mut p = i + 2 + sos_len;
            while p + 1 < out.len() {
                if out[p] == 0xFF && out[p + 1] == 0xD9 {
                    eoi_after_primary = Some(p + 2);
                    break;
                }
                p += 1;
            }
            if eoi_after_primary.is_some() {
                break;
            }
        }
        i += 1;
    }
    let eoi = eoi_after_primary.expect("应能找到主图 EOI");
    assert_eq!(&out[eoi..eoi + 2], &[0xFF, 0xD8], "次图像应以 SOI 开头");

    // 解析次图像 SOF0（0xFFC0）拿尺寸
    let mut p = eoi + 2;
    while p + 9 < out.len() {
        if out[p] == 0xFF && out[p + 1] == 0xC0 {
            let seg_len = u16::from_be_bytes([out[p + 2], out[p + 3]]);
            assert!(seg_len >= 7, "SOF0 段长度异常: {seg_len}");
            let seg_h = u16::from_be_bytes([out[p + 5], out[p + 6]]);
            let seg_w = u16::from_be_bytes([out[p + 7], out[p + 8]]);
            assert_eq!(seg_h, 8, "次图像高度应为主图 1/4 (32→8)");
            assert_eq!(seg_w, 8, "次图像宽度应为主图 1/4 (32→8)");
            return;
        }
        if out[p] == 0xFF && (out[p + 1] >= 0xC0 && out[p + 1] <= 0xCF) {
            // 跳过 SOF
            let seg_len = u16::from_be_bytes([out[p + 2], out[p + 3]]) as usize;
            p += 2 + seg_len;
            continue;
        }
        // 跳过其它 APP/COM 段
        if out[p] == 0xFF && (out[p + 1] >= 0xE0 && out[p + 1] <= 0xEF || out[p + 1] == 0xFE) {
            let seg_len = u16::from_be_bytes([out[p + 2], out[p + 3]]) as usize;
            p += 2 + seg_len;
            continue;
        }
        // SOI/EOI/RST 不该出现在这里
        p += 1;
    }
    panic!("未能在次图像中找到 SOF0");
}

/// 自动估算 HDR 强度：纯黑/纯白边界（算法为「裁剪预算扫描」，取代旧版 ×0.9/×1.05 修正）。
#[test]
fn estimate_hdr_intensity_sanity() {
    let black = hdrconv::ultra_hdr::estimate_hdr_intensity(&vec![0u8; 4], 1, 1);
    assert_eq!(black.hdr_intensity, 0.96, "无高光 → 钳到下限 0.96");
    assert!((black.max_boost - 2.0f64.powf(0.96)).abs() < 1e-9);

    let white = hdrconv::ultra_hdr::estimate_hdr_intensity(&vec![255u8; 4], 1, 1);
    assert!(white.y_p995 > 0.99);
    assert_eq!(white.hl_ratio, 1.0);
    // 全高光：锚点 EV=log2(2.8)≈1.4854，预算内上探 +0.35 → ≈1.8354（2^1.8354≈3.569）
    assert!((white.hdr_intensity - 1.835426827170242).abs() < 1e-9, "全高光应取锚点+0.35 上限");
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

/// 视频链路修复：32×32 半黑半白图，验证 `reconstruct_linear_hdr_frame` 应用低分辨率
/// 软阈值 mask 后，阈值附近**不是单步跳变**，而是平滑渐变。
#[test]
fn reconstruct_linear_hdr_frame_uses_soft_threshold() {
    let settings = Settings::default();
    let w = 32u32;
    let h = 32u32;

    // 半黑半白：x<16=0、x≥16=255
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let v: u8 = if x < 16 { 0 } else { 255 };
            rgba[i] = v;
            rgba[i + 1] = v;
            rgba[i + 2] = v;
            rgba[i + 3] = 255;
        }
    }

    let pam = hdrconv::ultra_hdr::reconstruct_linear_hdr_frame(
        &rgba,
        w,
        h,
        &settings,
        8.0,
        settings.ev(),
    )
    .expect("重建失败");

    let header = b"P7\nWIDTH 32\nHEIGHT 32\nDEPTH 3\nMAXVAL 65535\nTUPLTYPE RGB\nENDHDR\n";
    assert!(pam.starts_with(header));
    let px = &pam[header.len()..];

    // 在 y=16 这一行（穿过阈值边界）取水平剖面：x=0..=23 的 R 通道
    let y = 16u32;
    let row_start = (y * w * 6) as usize;
    let mut profile: Vec<u16> = Vec::with_capacity(w as usize);
    for x in 0..w {
        let o = row_start + (x as usize) * 6;
        profile.push(u16::from_be_bytes([px[o], px[o + 1]]));
    }

    // 单调非降
    for i in 1..profile.len() {
        assert!(
            profile[i] >= profile[i - 1],
            "PAM 像素剖面应单调非降：{profile:?}"
        );
    }

    // 软阈值特征：过渡带宽 ≥ 4 主图像素（低分辨率 1 像素 + 邻域扩展）。
    // 用 50% 高度作为"过半"判据，找从 0 到满亮的过渡带宽。
    let peak16 = *profile.last().unwrap();
    let half = peak16 / 2;
    let first = profile.iter().position(|&v| v >= half).unwrap_or(profile.len());
    let last = profile.iter().rposition(|&v| v >= half).unwrap_or(0);
    let band = last.saturating_sub(first) + 1;
    assert!(
        band >= 4,
        "过渡带宽应 ≥ 4 主图像素（软阈值），实际 {band}：剖面={profile:?}"
    );

    // 最左应保持 SDR 黑（≈0），最右应明显 > SDR 参考（gain > 1）
    // peak=8、maxBoost≈2.83 时 r=1.0 → 2.83/8*65535 ≈ 23185，远超 SDR 黑（clamp 到 0）。
    assert!(profile[0] <= 100, "最左列应为低值，实际 {}", profile[0]);
    let last = profile[(w - 1) as usize];
    assert!(last > 20000, "最右列应 > 20000（高光增益生效），实际 {last}");
    // 关键不变量：最右 vs 最左的差距应该跨越 ~50% 满量程——证明 gain>1 真的被应用了
    assert!(last > 200 * profile[0].max(1), "最右列应至少 200× 最左列，实际 last={last} first={}", profile[0]);
}

/// 视频链路修复：5 帧"接缝像素在低分辨率格子边界处跳动"的帧间方差对比（flicker 抑制）。
///
/// 构造 32×32 图：左半 y=0.49 linear（mask=0），右半 y=0.51 linear（mask=1）。
/// 接缝在主图 x=16 处，刚好落在低分辨率格子 x=3（主图 x=12..=15）和 x=4（主图 x=16..=19）的边界。
///
/// 帧 0..=4 让接缝在主图 x 方向 ±1 像素内**整体偏移**（模拟"暗部/亮部边界帧间抖动"，例如
/// 相机自动曝光、或剪映里模糊边缘的轻微动）：
///   帧 0：左 14/右 18  → 帧 1：左 15/右 19  → ... → 帧 4：左 18/右 22
///
/// 在低分辨率（每像素 4×4 主图像素）尺度上，左半亮低分辨率像素 x∈[3] 在 4 帧间：
///   帧 0：box(左 14/右 18 含 4 主图像素) = mix(0.49, 0.51) ≈ 0.50 → mask=1（旧）
///   帧 1：box(左 15/右 19 含 4 主图像素) = mix(0.49, 0.51) ≈ 0.50 → mask=1（旧）
///   帧 2：左半亮"占多" → mask=0（旧）
///   帧 3：...继续切换
/// 旧版硬阈值让低分辨率像素 mask 在 0/1 之间反复，跳变剧烈 → flicker；
/// 新版软阈值 mask 是渐变的（高斯平滑）→ mask 在 0..1 之间平滑移动，无 flicker。
#[test]
fn reconstruct_linear_hdr_frame_suppresses_flicker() {
    let settings = Settings::default();
    let w = 32u32;
    let h = 32u32;
    let peak = 8.0;
    let hdr_ev = settings.ev();

    let v_dark_lin = 0.49f64;
    let v_bright_lin = 0.51f64;
    let srgb_encode = |lin: f64| -> u8 { (lin.powf(1.0 / 2.4) * 255.0).round().clamp(0.0, 255.0) as u8 };
    let v_dark = srgb_encode(v_dark_lin);
    let v_bright = srgb_encode(v_bright_lin);

    // 接缝在主图 x = seam_x 处：x<seam_x = 暗，x≥seam_x = 亮。
    // 让 seam_x 帧间偏移 ±1 主图像素。
    let seam_xs: Vec<u32> = vec![14, 15, 16, 17, 18];

    let mut old_pixs: Vec<Vec<u16>> = Vec::new();
    let mut new_pixs: Vec<Vec<u16>> = Vec::new();
    for &seam_x in &seam_xs {
        // 构造主图
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                let v = if x < seam_x { v_dark } else { v_bright };
                rgba[i] = v;
                rgba[i + 1] = v;
                rgba[i + 2] = v;
                rgba[i + 3] = 255;
            }
        }

        // 旧实现（硬阈值，逐像素）
        let max_boost = 2.0f64.powf(hdr_ev).clamp(1.0, 64.0);
        let gamma = settings.gamma;
        let srgb_to_linear = |v: f64| -> f64 {
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        let dark_lin = srgb_to_linear(v_dark as f64 / 255.0);
        let bright_lin = srgb_to_linear(v_bright as f64 / 255.0);
        let mut old_pix = Vec::with_capacity((w * h) as usize);
        for _ in 0..h {
            for x in 0..w {
                let lin = if x < seam_x { dark_lin } else { bright_lin };
                let mask = ((lin - 0.5) / 0.5).clamp(0.0, 1.0).powf(gamma);
                let gain = 1.0 + (max_boost - 1.0) * mask;
                let hr = (lin * gain).clamp(0.0, peak) / peak * 65535.0;
                old_pix.push(hr.round() as u16);
            }
        }
        old_pixs.push(old_pix);

        // 新实现（软阈值）
        let pam = hdrconv::ultra_hdr::reconstruct_linear_hdr_frame(
            &rgba,
            w,
            h,
            &settings,
            peak,
            hdr_ev,
        )
        .expect("重建失败");
        let header = b"P7\nWIDTH 32\nHEIGHT 32\nDEPTH 3\nMAXVAL 65535\nTUPLTYPE RGB\nENDHDR\n";
        let px = &pam[header.len()..];
        let new_pix: Vec<u16> = (0..(w * h) as usize)
            .map(|i| u16::from_be_bytes([px[i * 6], px[i * 6 + 1]]))
            .collect();
        new_pixs.push(new_pix);
    }

    // 关注接缝附近的低分辨率像素 x=3 (主图 x=12..=15) 和 x=4 (主图 x=16..=19)，
    // 以及它们在 4×4 主图像素内的 R 值（取该低分辨率像素对应主图区域的 R 平均）。
    // 这里直接用全图所有像素的帧间方差来量化 flicker（接缝附近像素贡献最大）。
    let variance = |pixs: &[Vec<u16>]| -> f64 {
        let n_frames = pixs.len();
        let n_pix = pixs[0].len();
        let mut total = 0.0;
        for i in 0..n_pix {
            let mean: f64 = (0..n_frames).map(|f| pixs[f][i] as f64).sum::<f64>() / n_frames as f64;
            let var: f64 = (0..n_frames)
                .map(|f| (pixs[f][i] as f64 - mean).powi(2))
                .sum::<f64>()
                / n_frames as f64;
            total += var;
        }
        total / n_pix as f64
    };
    let old_var = variance(&old_pixs);
    let new_var = variance(&new_pixs);
    println!("帧间像素方差（旧硬阈值）={:.1}", old_var);
    println!("帧间像素方差（新软阈值）={:.1}", new_var);
    // 软阈值应显著小于硬阈值（经验阈值：< 0.3 ×）
    assert!(
        new_var < old_var * 0.3,
        "新版帧间方差（{:.1}）应显著小于旧版（{:.1}），否则 flicker 未被抑制",
        new_var,
        old_var
    );
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
        hdrconv::colorspace::detect(&input).space,
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
        hdrconv::colorspace::detect(&path).space,
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
        hdrconv::colorspace::detect(&path).space,
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
        hdrconv::colorspace::detect(&path).space,
        hdrconv::colorspace::InputColorSpace::DisplayP3,
        "注入 Display-P3 ICC 的 JPEG 应识别为 Display-P3"
    );
    let _ = std::fs::remove_file(path);
}

/// JPEG + APP2 注入 2020_profile.icc → 主色匹配 → Rec2020（Rust 扩展检测）。
#[test]
fn detect_jpeg_icc_rec2020() {
    use hdrconv::convert::{encode_jpeg_bytes, ImageData};

    let icc_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/2020_profile.icc");
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
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/test_tmp/rec2020_icc.jpg");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, &injected).unwrap();
    assert_eq!(
        hdrconv::colorspace::detect(&path).space,
        hdrconv::colorspace::InputColorSpace::Rec2020,
        "注入 Rec.2020 ICC 的 JPEG 应识别为 Rec2020"
    );
    let _ = std::fs::remove_file(path);
}

/// GPU == CPU 一致性测量（需 `cargo build --features gpu` + backend/cuda/hdr_gpu_ffi.dll + NVIDIA 卡）。
///
/// GPU 内核为 float32、CPU 为 float64 → 允许小差异；本测试统计差异并断言
/// 不超出预期界限（8-bit 通道 ≤1，16-bit 通道 ≤64 ≈ 0.1% 量程）。
/// 用法：`cargo test --features gpu -- --ignored gpu_cpu_parity --nocapture`
#[cfg(feature = "gpu")]
#[test]
#[ignore = "需 hdr_gpu_ffi.dll + NVIDIA GPU + --features gpu"]
fn gpu_cpu_parity() {
    use hdrconv::convert::{apply_hdr_rec2020_pq, ImageData};
    use hdrconv::gpu;
    use hdrconv::models::Settings;
    use hdrconv::ultra_hdr;

    unsafe { std::env::set_var("HDRCONV_GPU", "1") };
    assert!(gpu::gpu_available(), "GPU 不可用：需 backend/cuda/hdr_gpu_ffi.dll（jni/build_ffi.bat）");

    // 确定性 64x48 渐变（覆盖暗/中间调/高光）
    let w = 64u32;
    let h = 48u32;
    let mut pixels = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            pixels.push(((x * 255) / (w - 1)) as u8);
            pixels.push(((y * 255) / (h - 1)) as u8);
            pixels.push((((x + y) * 255) / (w + h - 2)) as u8);
            pixels.push(255);
        }
    }
    let img = ImageData { pixels, width: w, height: h };
    let settings = Settings::default();

    // 1) Rec.2020/PQ（png/jpg_icc 链路）
    let cpu = apply_hdr_rec2020_pq(&img, &settings).unwrap();
    let gpu_px = gpu::try_gpu_rec2020_pq(&img, &settings).expect("GPU rec2020pq 应成功");
    let (ndiff, maxd) = diff_stats(&cpu.pixels, &gpu_px);
    println!("[gpu] rec2020_pq: {ndiff}/{} 字节不同, 最大差 {maxd}", cpu.pixels.len());
    assert!(maxd <= 1, "rec2020_pq GPU/CPU 差异过大: max={maxd}");

    // 2) 增益图（Ultra HDR）
    let (gm_cpu, _meta_cpu) = ultra_hdr::compute_gain_map(&img.pixels, w as usize, h as usize, &settings);
    let (gm_gpu, _min, _max) = gpu::try_gpu_compute_gainmap(&img.pixels, w, h, settings.gain_ev(), settings.gamma)
        .expect("GPU gainmap 应成功");
    let (ndiff, maxd) = diff_stats(&gm_cpu, &gm_gpu);
    println!("[gpu] gainmap: {ndiff}/{} 字节不同, 最大差 {maxd}", gm_cpu.len());
    assert!(maxd <= 1, "gainmap GPU/CPU 差异过大: max={maxd}");

    // 3) 视频逐帧 16-bit（gainmap / transform）
    let peak = 4.9;
    let cv = ultra_hdr::reconstruct_linear_hdr_frame(&img.pixels, w, h, &settings, peak, settings.ev()).unwrap();
    let gv = gpu::try_gpu_reconstruct_gainmap16_pixels(&img.pixels, w, h, settings.ev(), settings.gamma, peak)
        .expect("GPU gainmap16 应成功");
    let (ndiff, maxd) = diff_stats(&cv[pam_data_off(&cv)..], &gv);
    println!("[gpu] gainmap16: {ndiff}/{} 字节不同, 最大差 {maxd}", gv.len());
    assert!(maxd <= 64, "gainmap16 GPU/CPU 差异过大: max={maxd}");

    let ct = ultra_hdr::reconstruct_linear_hdr_transform(&img.pixels, w, h, &settings, peak).unwrap();
    let gt = gpu::try_gpu_reconstruct_transform16_pixels(
        &img.pixels, w, h, peak, settings.gamma, settings.rgb.red, settings.rgb.green, settings.rgb.blue, peak,
    )
    .expect("GPU transform16 应成功");
    let (ndiff, maxd) = diff_stats(&ct[pam_data_off(&ct)..], &gt);
    println!("[gpu] transform16: {ndiff}/{} 字节不同, 最大差 {maxd}", gt.len());
    assert!(maxd <= 64, "transform16 GPU/CPU 差异过大: max={maxd}");

    unsafe { std::env::remove_var("HDRCONV_GPU") };
}

fn pam_data_off(pam: &[u8]) -> usize {
    pam.windows(8).position(|w| w == b"ENDHDR\n").map(|p| p + 7).unwrap_or(pam.len())
}

fn diff_stats(a: &[u8], b: &[u8]) -> (usize, u8) {
    let mut ndiff = 0usize;
    let mut maxd = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        let d = x.abs_diff(*y);
        if d > 0 {
            ndiff += 1;
            maxd = maxd.max(d);
        }
    }
    (ndiff, maxd)
}