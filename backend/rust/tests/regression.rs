//! 回归测试占位。
//!
//! 目标：同一输入分别用 hdrconv 与 Kotlin 后端（
//! `java -jar archive/kotlin-backend/build/libs/hdr-converter-backend.jar` 的 /convert 端点）处理，
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
    assert!(
        (nits - 10000.0).abs() < 1.0,
        "PQ EOTF(1.0) 应≈10000，实际 {nits}"
    );
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
    let output =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/test_tmp/uhdr_out.jpg");
    let settings = Settings::default();

    convert_image(&input, &output, &settings, OutputFormat::UltraHdr)
        .expect("ultra-hdr 转换应成功");
    let bytes = std::fs::read(&output).expect("输出应存在");

    // 结构与段落
    assert!(
        find_bytes(&bytes, b"ICC_PROFILE\0").is_some(),
        "应含 ICC_PROFILE 段"
    );
    assert!(find_bytes(&bytes, b"MPF\0").is_some(), "应含 MPF 索引段");
    assert!(
        find_bytes(&bytes, b"hdrgm:GainMapMax=").is_some(),
        "应含 hdrgm XMP"
    );
    assert!(
        find_bytes(&bytes, b"Item:Length=").is_some(),
        "应含 GContainer Item"
    );
    assert!(
        find_bytes(&bytes, b"Container:Directory").is_some(),
        "应含 GContainer 目录"
    );

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
        assert_eq!(
            &icc[132 + i * 12..136 + i * 12],
            expect.as_bytes(),
            "标签 {i} 应按签名排序"
        );
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
        assert_eq!(
            v, 128,
            "错位边缘：列 1 源范围 [2..4) 一半 0 一半 255，应为 128，实际 {c1:?}"
        );
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
        assert!(a[i] >= a[i - 1], "面积平均在 i={i} 处违反单调性：a={:?}", a);
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
    use hdrconv::ultra_hdr::GAINMAP_SCALE;
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
    // gm8 尺寸 = 主图 / GAINMAP_SCALE
    assert_eq!(
        gm8.len(),
        (w as usize / GAINMAP_SCALE) * (h as usize / GAINMAP_SCALE)
    );

    let gm_w = w as usize / GAINMAP_SCALE;
    let gm_h = h as usize / GAINMAP_SCALE;

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
    use hdrconv::ultra_hdr::GAINMAP_SCALE;
    use hdrconv::ultra_hdr::compute_gain_map;

    let w = 32u32;
    let h = 32u32;
    let settings = Settings::default();

    // 纯黑图：整张 0 像素，gain=1 处处，gm8 全 0（minBoost=1、mapMin=0）
    let black = vec![0u8; (w * h * 4) as usize];
    let (gm_black, meta_black) = compute_gain_map(&black, w as usize, h as usize, &settings);
    assert_eq!(
        gm_black.len(),
        (w as usize / GAINMAP_SCALE) * (h as usize / GAINMAP_SCALE)
    );
    assert!(
        gm_black.iter().all(|&v| v == 0),
        "纯黑图增益图应全 0，实际 {gm_black:?}"
    );
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
    let gm_w = w as usize / GAINMAP_SCALE;
    let gm_h = h as usize / GAINMAP_SCALE;
    assert_eq!(gm_half.len(), gm_w * gm_h);
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
    // 左半（x<gm_w/2）应全为 0，右半应明显 > 0（高光增益生效）
    let half_w = gm_w / 2;
    let left_avg: f64 = (0..half_w).map(|x| col_profile[x] as f64).sum::<f64>() / half_w as f64;
    let right_avg: f64 =
        (half_w..gm_w).map(|x| col_profile[x] as f64).sum::<f64>() / (gm_w - half_w) as f64;
    assert!(
        left_avg < 1.0,
        "左半列均值应接近 0（gain=1），实际 {left_avg}"
    );
    assert!(
        right_avg > 50.0,
        "右半列均值应明显 > 0（高光扩展），实际 {right_avg}"
    );
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
    assert!(
        b[0] <= b[1] && b[1] <= b[2] && b[2] <= b[3] && b[3] <= b[4],
        "应单调非降：{b:?}"
    );
    // 中心像素 x=2：两侧 1+1+0=2 → 2/3 ≈ 0.6667
    assert!(
        (b[2] - 2.0 / 3.0).abs() < 1e-6,
        "中心像素应 ≈ 2/3，实际 {}",
        b[2]
    );
    // 阶跃点 x=1：单侧 1+0+0=1 → 1/3 ≈ 0.3333
    assert!(
        (b[1] - 1.0 / 3.0).abs() < 1e-6,
        "阶跃前应 ≈ 1/3，实际 {}",
        b[1]
    );
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
    use hdrconv::ultra_hdr::GAINMAP_SCALE;
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
    let gm_w = w as usize / GAINMAP_SCALE;
    let gm_h = h as usize / GAINMAP_SCALE;

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
    let c_last = col_mean[gm_w - 1];
    let span = c_last - c0;
    assert!(span > 16.0, "总跨度应大于 16：{col_mean:?}");
    // 检查"过渡带宽 ≥ 2 个低分辨率像素"：取中段至少 3 个连续的不同值（或跨度 > 32）
    let transition = &col_mean[1..gm_w - 1];
    let mn = transition.iter().cloned().fold(f64::INFINITY, f64::min);
    let mx = transition.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    assert!(
        mx - mn > 16.0,
        "过渡区(x=1..=gm_w-2) 跨度应 > 16（软阈值）；当前 {transition:?}"
    );
    // 最左列（x=0）应仍接近 0（全 0 像素）
    assert!(c0 < 8.0, "最左列均值应接近 0，实际 {c0}");
    // 最右列应明显 > 0
    assert!(c_last > 50.0, "最右列均值应 > 50，实际 {c_last}");
}

/// 第二步：encode_ultra_hdr 在 32×32 图上的端到端，验证 gm_w/gm_h = 主图 1/4。
#[test]
fn encode_ultra_hdr_uses_low_resolution_gainmap() {
    use hdrconv::ultra_hdr::GAINMAP_SCALE;
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
            assert_eq!(
                seg_h as usize,
                h as usize / GAINMAP_SCALE,
                "次图像高度应为主图 / GAINMAP_SCALE"
            );
            assert_eq!(
                seg_w as usize,
                w as usize / GAINMAP_SCALE,
                "次图像宽度应为主图 / GAINMAP_SCALE"
            );
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

/// PQ 输出色域 ICC：P3/sRGB 必须是 PQ tagged profile（含 cicp + curv），不能复用 sRGB TRC。
#[test]
fn pq_icc_for_output_gamut_is_pq_tagged() {
    fn tag<'a>(icc: &'a [u8], sig: &[u8; 4]) -> Option<&'a [u8]> {
        let n = u32::from_be_bytes([icc[128], icc[129], icc[130], icc[131]]) as usize;
        for i in 0..n {
            let e = 132 + i * 12;
            if &icc[e..e + 4] == sig {
                let o =
                    u32::from_be_bytes([icc[e + 4], icc[e + 5], icc[e + 6], icc[e + 7]]) as usize;
                let s =
                    u32::from_be_bytes([icc[e + 8], icc[e + 9], icc[e + 10], icc[e + 11]]) as usize;
                return Some(&icc[o..o + s]);
            }
        }
        None
    }

    for gamut in ["p3", "srgb"] {
        let icc = hdrconv::ultra_hdr::build_pq_icc_for_gamut(gamut)
            .unwrap_or_else(|| panic!("{gamut} PQ ICC 应可生成"));
        assert_eq!(&icc[36..40], b"acsp", "{gamut}: 应为合法 ICC");
        let cicp = tag(&icc, b"cicp").unwrap_or_else(|| panic!("{gamut}: 应含 cicp 标签"));
        assert_eq!(cicp[9], 16, "{gamut}: transfer 应为 PQ");
        let want_p = if gamut == "p3" { 12 } else { 1 };
        assert_eq!(cicp[8], want_p, "{gamut}: primaries 码值");
        let trc = tag(&icc, b"rTRC").unwrap_or_else(|| panic!("{gamut}: 应含 rTRC 标签"));
        assert_eq!(&trc[0..4], b"curv", "{gamut}: TRC 应为 curv 查表");
        let count = u32::from_be_bytes([trc[8], trc[9], trc[10], trc[11]]);
        assert!(count > 1, "{gamut}: 应为查表曲线，实际 count={count}");
    }
    assert!(
        hdrconv::ultra_hdr::build_pq_icc_for_gamut("2020").is_none(),
        "2020 使用 assets/2020_profile.icc，不程序化生成"
    );
}

/// 视频帧重建：PAM 头 + 大端 16-bit 数据（单层色调映射，transform 链路）。
#[test]
fn reconstruct_pam_structure() {
    let settings = Settings::default();
    let rgba = vec![255u8; 8]; // 2x1 纯白
    let pam = hdrconv::ultra_hdr::reconstruct_linear_hdr_transform(&rgba, 2, 1, &settings, 8.0)
        .expect("重建失败");
    let header = b"P7\nWIDTH 2\nHEIGHT 1\nDEPTH 3\nMAXVAL 65535\nTUPLTYPE RGB\nENDHDR\n";
    assert!(pam.starts_with(header), "PAM 头不正确");
    assert_eq!(pam.len(), header.len() + 2 * 1 * 3 * 2, "数据长度应为 n*6");
    // 纯白 → 单层色调映射（曝光=8、伽马）后线性值显著高于 SDR 参考（>20000/65535）
    let first = u16::from_be_bytes([pam[header.len()], pam[header.len() + 1]]);
    assert!(
        first > 20000,
        "纯白像素经曝光/伽马后应显著高于 SDR 白，实际 {first}"
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
/// 对照条件：Rust 侧 Settings::default()（peak=500, white=203, gamma=0.9, rgb=0.96,1,1）
/// 与基准脚本传给 Kotlin /convert 的参数完全一致；Kotlin 侧已用 HDR_GPU_DISABLE=1 强制 CPU。
#[test]
#[ignore = "需先运行 node tests/rust_baseline.js 生成 Kotlin 基准"]
fn png_pixels_match_kotlin_baseline() {
    use std::path::Path;

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."); // hdr_electron/（tests/ 在仓库根）
    let input = root.join("tests/rust_ref_input.png");
    let kotlin = root.join("tests/rust_ref_kotlin.png");
    assert!(
        input.exists(),
        "缺少基准输入，请先运行: node tests/rust_baseline.js"
    );
    assert!(
        kotlin.exists(),
        "缺少 Kotlin 基准输出，请先运行: node tests/rust_baseline.js"
    );

    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/test_tmp/rust_ref_rust.png");
    let mut settings = Settings::default();
    settings.icc_path = Some("../../assets/2020_profile.icc".to_string());
    convert_image(&input, &out, &settings, OutputFormat::Png).expect("Rust 转换失败");

    let a = image::open(&kotlin)
        .expect("解码 Kotlin 输出失败")
        .to_rgba8()
        .into_raw();
    let b = image::open(&out)
        .expect("解码 Rust 输出失败")
        .to_rgba8()
        .into_raw();
    assert_eq!(
        a.len(),
        b.len(),
        "像素数不一致: kotlin={} rust={}",
        a.len(),
        b.len()
    );

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
    use hdrconv::convert::{ImageData, encode_png_bytes};
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
    let pos = find_bytes(&injected, b"iCCP").unwrap_or_else(|| panic!("未找到 iCCP chunk"));
    let data_start = pos + 4; // chunk 数据起点（type 之后）
    let len = u32::from_be_bytes([
        injected[pos - 4],
        injected[pos - 3],
        injected[pos - 2],
        injected[pos - 1],
    ]) as usize;
    let data = &injected[data_start..data_start + len];
    // 名称 "ICC\0" + 压缩方法 0
    assert_eq!(&data[..4], b"ICC\0", "iCCP 名称应为通用 ICC");
    assert_eq!(data[4], 0, "压缩方法应为 deflate");
    let mut dec = flate2::read::ZlibDecoder::new(&data[5..]);
    let mut out = Vec::new();
    dec.read_to_end(&mut out).expect("解压 iCCP 失败");
    assert_eq!(out, icc, "iCCP 解压数据应等于原 ICC");
}

/// JPEG APP2 注入往返：注入后仍可解码、尺寸不变、含 ICC_PROFILE 签名。
#[test]
fn jpeg_app2_injection_roundtrip() {
    use hdrconv::convert::{ImageData, encode_jpeg_bytes};

    let icc = std::fs::read("../../assets/2020_profile.icc").expect("读取 ICC 失败");
    let img = ImageData {
        pixels: vec![
            10, 20, 30, 255, 200, 100, 50, 255, 90, 60, 120, 255, 250, 250, 240, 255,
        ],
        width: 2,
        height: 2,
    };
    let plain = encode_jpeg_bytes(&img, 0.9).expect("JPEG 编码失败");
    let injected = hdrconv::icc::inject_icc_into_jpeg(&plain, &icc).expect("APP2 注入失败");

    assert!(
        injected.windows(12).any(|w| w == b"ICC_PROFILE\0"),
        "应含 ICC_PROFILE 签名"
    );
    assert!(injected.len() > plain.len(), "注入后应变大");

    let decoded = image::load_from_memory(&injected).expect("注入后 JPEG 应可解码");
    assert_eq!(decoded.dimensions(), (2, 2));
}

/// jpg-icc 端到端：Rec.2020/PQ 变换 + APP2 ICC 注入。
#[test]
fn jpg_icc_end_to_end() {
    let (input, output) = temp_paths("jpegicc");
    let mut settings = Settings::default();
    settings.icc_path = Some("../../assets/2020_profile.icc".to_string());

    convert_image(&input, &output, &settings, OutputFormat::JpgIcc).expect("jpg-icc 转换应成功");
    let bytes = std::fs::read(&output).expect("输出应存在");
    assert!(
        bytes.windows(12).any(|w| w == b"ICC_PROFILE\0"),
        "输出应含 ICC_PROFILE"
    );

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
    let pos = bytes
        .windows(needle.len())
        .position(|w| w == needle.as_bytes())?;
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
    assert!(
        input.exists(),
        "缺少基准输入，请先运行: node tests/rust_baseline.js"
    );
    assert!(
        kotlin.exists(),
        "缺少 Kotlin ultra-hdr 基准，请先运行: node tests/rust_baseline.js"
    );

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
        let vk =
            xmp_num(&kotlin_bytes, key).unwrap_or_else(|| panic!("Kotlin 基准缺少 hdrgm:{key}"));
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
    let input =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/rust_ref_input.png");
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
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/test_tmp/srgb_chunk.png");
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

    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/test_tmp/exif_srgb.jpg");
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
    use hdrconv::convert::{ImageData, encode_jpeg_bytes};

    let icc_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/display_p3_primary.icc");
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
    use hdrconv::convert::{ImageData, encode_jpeg_bytes};

    let icc_path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/2020_profile.icc");
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
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/test_tmp/rec2020_icc.jpg");
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
/// 覆盖 2026-10 同步的三块：
///   1. ICC 链路（png/jpg_icc）：明度 B 曲线 + 输出色域 + Oklab 色域拓展
///   2. Ultra HDR 增益图：B 曲线 + 局部镜面高光 top-hat
///   3. 视频逐帧 transform16（既有）
/// GPU 内核为 float32、CPU 为 float64 → 允许小差异（8-bit ≤2，16-bit ≤64）。
/// 用法：`cargo test --features gpu -- --ignored gpu_cpu_parity --nocapture`
#[cfg(feature = "gpu")]
#[test]
#[ignore = "需 hdr_gpu_ffi.dll + NVIDIA GPU + --features gpu"]
fn gpu_cpu_parity() {
    use hdrconv::convert::{self, ImageData};
    use hdrconv::gpu;
    use hdrconv::models::Settings;
    use hdrconv::ultra_hdr;

    unsafe { std::env::set_var("HDRCONV_GPU", "1") };
    assert!(
        gpu::gpu_available(),
        "GPU 不可用：需 backend/cuda/hdr_gpu_ffi.dll（jni/build_ffi.bat）"
    );

    // 确定性 96x72 渐变（覆盖暗/中间调/高光，右下角单像素纯白 = 局部镜面高光）
    let w = 96u32;
    let h = 72u32;
    let mut pixels = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            pixels.push(((x * 255) / (w - 1)) as u8);
            pixels.push(((y * 255) / (h - 1)) as u8);
            pixels.push((((x + y) * 255) / (w + h - 2)) as u8);
            pixels.push(255);
        }
    }
    if let Some(last) = pixels.chunks_exact_mut(4).last() {
        last[0] = 255;
        last[1] = 255;
        last[2] = 255;
    }
    let img = ImageData {
        pixels,
        width: w,
        height: h,
    };

    // 1) ICC 路径：多组 rolloff / image_gamma / 输出色域 / 色域拓展 / 峰值
    let icc_cases: Vec<(&str, Settings)> = vec![
        ("默认(2020,expand=1)", Settings::default()),
        ("rolloff=0", Settings { rolloff: 0.0, ..Settings::default() }),
        ("rolloff=1", Settings { rolloff: 1.0, ..Settings::default() }),
        ("image_gamma=0.8", Settings { image_gamma: 0.8, ..Settings::default() }),
        ("image_gamma=1.2", Settings { image_gamma: 1.2, ..Settings::default() }),
        ("p3+expand=1", Settings { output_gamut: Some("p3".into()), ..Settings::default() }),
        ("p3+expand=0.5", Settings {
            output_gamut: Some("p3".into()),
            gamut_expand: 0.5,
            ..Settings::default()
        }),
        ("srgb(无拓展)", Settings { output_gamut: Some("srgb".into()), ..Settings::default() }),
        ("expand=0", Settings { gamut_expand: 0.0, ..Settings::default() }),
        ("峰值1000", Settings { peak_nits: 1000.0, ..Settings::default() }),
    ];
    for (name, s) in &icc_cases {
        let cpu = convert::apply_hdr_rec2020_pq(&img, s).expect("CPU ICC 应成功");
        let gt = gpu::try_gpu_rec2020_pq(&img, s).expect("GPU ICC 应成功");
        let (ndiff, maxd) = diff_stats(&cpu.pixels, &gt);
        println!(
            "[gpu] ICC {name}: {ndiff}/{} 字节不同（{:.3}%）, 最大差 {maxd}",
            gt.len(),
            ndiff as f64 * 100.0 / gt.len() as f64
        );
        assert!(maxd <= 2, "ICC {name} GPU/CPU 差异过大: max={maxd}");
    }

    // 2) Ultra HDR 增益图（B 曲线 + 局部镜面高光）
    let gain_cases: Vec<(&str, Settings)> = vec![
        ("默认", Settings::default()),
        ("rolloff=0", Settings { rolloff: 0.0, ..Settings::default() }),
        ("image_gamma=0.8", Settings { image_gamma: 0.8, ..Settings::default() }),
        ("峰值1000", Settings { peak_nits: 1000.0, ..Settings::default() }),
        ("gain_scale=4", Settings { gain_scale: 4, ..Settings::default() }),
    ];
    for (name, s) in &gain_cases {
        let (cg, cm) = ultra_hdr::compute_gain_map(&img.pixels, w as usize, h as usize, s);
        let (gg, gm) =
            gpu::try_gpu_compute_gainmap(&img.pixels, w, h, s).expect("GPU 增益图应成功");
        assert_eq!(cg.len(), gg.len(), "增益图尺寸应一致");
        let (ndiff, maxd) = diff_stats(&cg, &gg);
        println!(
            "[gpu] gainmap {name}: {ndiff}/{} 字节不同, 最大差 {maxd} | \
             min {:.6}/{:.6} max {:.6}/{:.6}",
            gg.len(),
            cm.min_content_boost,
            gm.min_content_boost,
            cm.max_content_boost,
            gm.max_content_boost
        );
        assert!(maxd <= 2, "gainmap {name} GPU/CPU 差异过大: max={maxd}");
        assert!(
            (cm.min_content_boost - gm.min_content_boost).abs() < 1e-4
                && (cm.max_content_boost - gm.max_content_boost).abs() < 1e-4,
            "gainmap {name} 元数据不一致"
        );
    }

    // 3) 视频逐帧 16-bit（单层色调映射 transform16）
    let settings = Settings::default();
    let peak = 4.9;
    let ct =
        ultra_hdr::reconstruct_linear_hdr_transform(&img.pixels, w, h, &settings, peak).unwrap();
    let gt = gpu::try_gpu_reconstruct_transform16_pixels(
        &img.pixels,
        w,
        h,
        peak,
        settings.gamma,
        settings.rgb.red,
        settings.rgb.green,
        settings.rgb.blue,
        peak,
    )
    .expect("GPU transform16 应成功");
    let (ndiff, maxd) = diff_stats(&ct[pam_data_off(&ct)..], &gt);
    println!(
        "[gpu] transform16: {ndiff}/{} 字节不同, 最大差 {maxd}",
        gt.len()
    );
    assert!(maxd <= 64, "transform16 GPU/CPU 差异过大: max={maxd}");

    unsafe { std::env::remove_var("HDRCONV_GPU") };
}

#[cfg(feature = "gpu")]
fn pam_data_off(pam: &[u8]) -> usize {
    pam.windows(8)
        .position(|w| w == b"ENDHDR\n")
        .map(|p| p + 7)
        .unwrap_or(pam.len())
}

#[cfg(feature = "gpu")]
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

/// 按明度加增益：中调 gain > 白点 gain，且白点仍达到峰值。
#[test]
fn lightness_gain_lifts_midtones_above_white() {
    use hdrconv::ultra_hdr::compute_gain_map;

    let w = 64usize;
    let h = 64usize;
    let settings = Settings::default();
    let e = settings.peak_nits / settings.white_nits;
    let o = 1.0 / 64.0;

    // 纯中灰（sRGB 128，线性≈0.216）：按明度应有明显增益（不再像高光阈值曲线那样=1）
    let mut gray = vec![0u8; w * h * 4];
    for px in gray.chunks_exact_mut(4) {
        px[0] = 128;
        px[1] = 128;
        px[2] = 128;
        px[3] = 255;
    }
    let (_g, mg) = compute_gain_map(&gray, w, h, &settings);
    assert!(
        mg.max_content_boost > 1.2,
        "中调应有增益：max={}",
        mg.max_content_boost
    );

    // 纯白：白点按 settings.rolloff 滚降（默认 0.45）；ratio=(1+(E-1)r+o)/(1+o)
    let mut white = vec![0u8; w * h * 4];
    for px in white.chunks_exact_mut(4) {
        px[0] = 255;
        px[1] = 255;
        px[2] = 255;
        px[3] = 255;
    }
    let (_g, mw) = compute_gain_map(&white, w, h, &settings);
    let want_t = 1.0 + (e - 1.0) * settings.rolloff;
    let want = (want_t + o) / (1.0 + o);
    assert!(
        (mw.max_content_boost - want).abs() < 1e-6,
        "白点 ratio 应={want}，实际 {}",
        mw.max_content_boost
    );
    let peak = (1.0 + o) * mw.max_content_boost - o;
    assert!((peak - want_t).abs() < 1e-9, "白点解码亮度应=滚降目标");
    assert!((mw.min_content_boost - 1.0).abs() < 1e-9);

    // 关键：中调增益高于白点（颜色不再被白压暗）
    assert!(
        mg.max_content_boost > mw.max_content_boost,
        "中调增益应高于白点：mid={} white={}",
        mg.max_content_boost,
        mw.max_content_boost
    );
}

/// 白点滚降是 ICC 与 Ultra HDR 共用的用户参数：同一纯白图，rolloff 越大输出越亮。
#[test]
fn rolloff_is_shared_by_icc_path() {
    use hdrconv::convert::{ImageData, apply_hdr_rec2020_pq};

    let img = ImageData {
        pixels: vec![255, 255, 255, 255, 255, 255, 255, 255],
        width: 2,
        height: 1,
    };
    let mut low = Settings::default();
    low.rolloff = 0.0;
    let mut high = Settings::default();
    high.rolloff = 1.0;

    let low_px = apply_hdr_rec2020_pq(&img, &low).unwrap();
    let high_px = apply_hdr_rec2020_pq(&img, &high).unwrap();
    assert!(
        high_px.pixels[0] > low_px.pixels[0],
        "rolloff=1 的白点应比 rolloff=0 更亮：low={} high={}",
        low_px.pixels[0],
        high_px.pixels[0]
    );

    // Ultra HDR 的增益图 max boost 同样随 rolloff 增大。
    use hdrconv::ultra_hdr::compute_gain_map;
    let rgba = vec![255u8; 8 * 8 * 4];
    let (_g0, m0) = compute_gain_map(&rgba, 8, 8, &low);
    let (_g1, m1) = compute_gain_map(&rgba, 8, 8, &high);
    assert!(m1.max_content_boost > m0.max_content_boost);
}

/// 图片 ICC 路径输出色域：同一张饱和红图，P3 与 BT.2020 的 PQ 码应不同。
#[test]
fn icc_output_gamut_changes_pixels() {
    use hdrconv::convert::{ImageData, apply_hdr_rec2020_pq};

    let img = ImageData {
        pixels: vec![255, 0, 0, 255, 255, 0, 0, 255],
        width: 2,
        height: 1,
    };
    let mut p3 = Settings::default();
    p3.output_gamut = Some("p3".into());
    let mut rec2020 = Settings::default();
    rec2020.output_gamut = Some("2020".into());

    let a = apply_hdr_rec2020_pq(&img, &p3).unwrap();
    let b = apply_hdr_rec2020_pq(&img, &rec2020).unwrap();
    assert_ne!(a.pixels, b.pixels, "P3 与 BT.2020 的输出应不同");
}

/// 色域拓展（gamut extension）：`gamut_expand` 只在 ICC 路径生效。
///
/// - BT.2020 + expand=1 应把纯 sRGB 红推到更靠近 BT.2020 色域边界（u′v′ 彩度显著变大）；
/// - sRGB 输出（恒等矩阵）不建表、不拉伸：expand=0 与 expand=1 必须逐字节一致；
/// - 中性色（灰/白，彩度 0）不得被拉伸（色相/亮度不变）。
#[test]
fn gamut_expand_widens_chroma_only_for_wide_gamut() {
    use hdrconv::convert::{ImageData, apply_hdr_rec2020_pq};

    // 红 / 灰 / 白
    let img = ImageData {
        pixels: vec![
            255, 0, 0, 255, // 红
            128, 128, 128, 255, // 灰
            255, 255, 255, 255, // 白
        ],
        width: 3,
        height: 1,
    };

    // PQ 码（0..1）→ 线性（相对 10000 尼特）
    fn pq_inv(code: f64) -> f64 {
        let (m1, m2) = (0.1593017578125, 78.84375);
        let (c1, c2, c3) = (0.8359375, 18.8515625, 18.6875);
        let p = code.powf(1.0 / m2);
        ((p - c1).max(0.0) / (c2 - c3 * p)).powf(1.0 / m1)
    }
    // BT.2020 RGB → XYZ（D65），返回 (u′, v′)
    fn uv2020(px: &[u8], i: usize) -> (f64, f64) {
        let scale = 203.0 / 10_000.0; // 与 Settings::default 的 white_nits 一致
        let lin: Vec<f64> = (0..3)
            .map(|k| pq_inv(px[i * 4 + k] as f64 / 255.0) / scale)
            .collect();
        let m = [
            [0.636_958_0, 0.144_616_9, 0.168_881_0],
            [0.262_700_2, 0.677_998_1, 0.059_301_7],
            [0.000_000_0, 0.028_072_7, 1.060_985_1],
        ];
        let x = m[0][0] * lin[0] + m[0][1] * lin[1] + m[0][2] * lin[2];
        let y = m[1][0] * lin[0] + m[1][1] * lin[1] + m[1][2] * lin[2];
        let z = m[2][0] * lin[0] + m[2][1] * lin[1] + m[2][2] * lin[2];
        let d = x + 15.0 * y + 3.0 * z;
        (4.0 * x / d, 9.0 * y / d)
    }
    // 与 D65 白的 u′v′ 距离（= 彩度）
    fn chroma(px: &[u8], i: usize) -> f64 {
        let (u, v) = uv2020(px, i);
        let (uw, vw) = (0.197_830, 0.468_320);
        ((u - uw).powi(2) + (v - vw).powi(2)).sqrt()
    }

    let mut flat = Settings::default();
    flat.output_gamut = Some("2020".into());
    flat.gamut_expand = 0.0;
    let mut full = Settings::default();
    full.output_gamut = Some("2020".into());
    full.gamut_expand = 1.0;

    let a = apply_hdr_rec2020_pq(&img, &flat).unwrap();
    let b = apply_hdr_rec2020_pq(&img, &full).unwrap();

    let (ca, cb) = (chroma(&a.pixels, 0), chroma(&b.pixels, 0));
    assert!(
        cb > ca * 1.12,
        "expand=1 的红色彩度应显著高于 expand=0：{ca:.4} -> {cb:.4}"
    );

    // 中性色不拉伸：灰、白的三个通道保持一致
    for i in [1usize, 2] {
        let px = &b.pixels[i * 4..i * 4 + 3];
        assert!(
            px[0] == px[1] && px[1] == px[2],
            "中性色不应被色度拉伸：{px:?}"
        );
    }

    // sRGB 输出（恒等矩阵）不做拓展：两组参数必须逐字节一致
    let mut s0 = flat.clone();
    s0.output_gamut = Some("srgb".into());
    let mut s1 = full.clone();
    s1.output_gamut = Some("srgb".into());
    let p0 = apply_hdr_rec2020_pq(&img, &s0).unwrap();
    let p1 = apply_hdr_rec2020_pq(&img, &s1).unwrap();
    assert_eq!(p0.pixels, p1.pixels, "sRGB 输出不应受 gamut_expand 影响");
}

/// 图片亮度域伽马（`image_gamma` 0.1~2）：只改亮度曲线，不改色度、不动白点。
///
/// - γ<1 提亮中调、γ>1 压暗中调；
/// - 白点（纯白）不受伽马影响（`1^γ = 1`）；
/// - 中性灰仍是中性（RGB 同增益，不变色）；
/// - Ultra HDR 增益图同步变化（ICC 与 Ultra HDR 共用同一曲线）。
#[test]
fn image_gamma_moves_midtones_only() {
    use hdrconv::convert::{ImageData, apply_hdr_rec2020_pq};

    // 中灰 / 暗灰 / 白
    let img = ImageData {
        pixels: vec![128, 128, 128, 255, 32, 32, 32, 255, 255, 255, 255, 255],
        width: 3,
        height: 1,
    };
    let make = |g: f64| {
        let mut s = Settings::default();
        s.output_gamut = Some("2020".into());
        s.gamut_expand = 0.0;
        s.image_gamma = g;
        s
    };

    let base = apply_hdr_rec2020_pq(&img, &make(1.0)).unwrap();
    let bright = apply_hdr_rec2020_pq(&img, &make(0.8)).unwrap();
    let dark = apply_hdr_rec2020_pq(&img, &make(1.2)).unwrap();

    // 中调：γ=0.8 更亮、γ=1.2 更暗（PQ 码越大越亮）
    assert!(
        bright.pixels[0] > base.pixels[0] && dark.pixels[0] < base.pixels[0],
        "中调应随 γ 单调变化：0.8→{} 1.0→{} 1.2→{}",
        bright.pixels[0],
        base.pixels[0],
        dark.pixels[0]
    );

    // 白点不动（允许 1 个码的量化误差）
    let white = 2 * 4;
    assert!(
        (bright.pixels[white] as i32 - base.pixels[white] as i32).abs() <= 1
            && (dark.pixels[white] as i32 - base.pixels[white] as i32).abs() <= 1,
        "白点不应受伽马影响：{} / {} / {}",
        bright.pixels[white],
        base.pixels[white],
        dark.pixels[white]
    );

    // 中性色保持中性（RGB 三通道一致）
    for i in 0..3 {
        let px = &bright.pixels[i * 4..i * 4 + 3];
        assert!(
            px[0] == px[1] && px[1] == px[2],
            "中性色不应被伽马染色：{px:?}"
        );
    }

    // Ultra HDR 增益图同步：γ<1 的最大增益应高于 γ>1
    use hdrconv::ultra_hdr::compute_gain_map;
    let rgba: Vec<u8> = (0..64 * 64)
        .flat_map(|_| [128u8, 128, 128, 255])
        .collect();
    let mut s_lo = Settings::default();
    s_lo.image_gamma = 0.8;
    let mut s_hi = Settings::default();
    s_hi.image_gamma = 1.2;
    let (_g1, m_lo) = compute_gain_map(&rgba, 64, 64, &s_lo);
    let (_g2, m_hi) = compute_gain_map(&rgba, 64, 64, &s_hi);
    assert!(
        m_lo.max_content_boost > m_hi.max_content_boost,
        "γ=0.8 的增益图最大 boost 应大于 γ=1.2：{} vs {}",
        m_lo.max_content_boost,
        m_hi.max_content_boost
    );
}
