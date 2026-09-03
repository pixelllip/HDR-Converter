//! 临时调试工具：用同一张主图，按两种"增益图下采样语义"产生 480×270 增益图，
//! 并把两者 + 差值 dump 成 PNG。
//!
//! - **第一步（旧）**：全分辨率逐像素算 mask/gain（亮度的硬阈值） → ratio map，
//!   再用 2-tap 双线性 decimation 抽到 1/4（缺乏低通预滤波 → aliasing 锐阶跃）。
//! - **第二步（新，当前主链路）**：box-average 下采样主图到 1/4 → 在低分辨率上算
//!   mask/gain（亮度阈值在低分辨率上自然平滑） → 8-bit 量化。
//!
//! 用法：
//!   cargo run --example dump_gainmap_compare -- <input.png> <out_dir>
//!
//! 不进主链路，仅作视觉/数值回归脚本。

use std::path::PathBuf;
use hdrconv::models::Settings;
use hdrconv::ultra_hdr::{
    compute_gain_map, downscale_area_average_box, downscale_bilinear,
};
use image::ImageBuffer;

/// 模拟"第一步"：全分辨率逐像素 ratio + 双线性 decimation。
fn step1_fullres_then_bilinear(rgba: &[u8], w: usize, h: usize, settings: &Settings) -> Vec<u8> {
    let hdr_intensity = settings.gain_ev();
    let gamma = settings.gamma;
    let user_max_boost = 2.0f64.powf(hdr_intensity).clamp(1.0, 64.0);
    let white_nits = settings.white_nits;
    let peak_cap = (settings.peak_nits / white_nits).max(1.0);
    let max_boost = user_max_boost.min(peak_cap);
    let highlight_start = 0.5f64;
    let offset = 1.0 / 64.0;

    let n = w * h;
    let mut gain = Vec::with_capacity(n);
    for i in 0..n {
        let base = i * 4;
        let r = srgb_to_linear(rgba[base] as f64 / 255.0);
        let g = srgb_to_linear(rgba[base + 1] as f64 / 255.0);
        let b = srgb_to_linear(rgba[base + 2] as f64 / 255.0);
        let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        let mask = ((y - highlight_start) / (1.0 - highlight_start))
            .clamp(0.0, 1.0)
            .powf(gamma);
        let gain_per_pix = 1.0 + (max_boost - 1.0) * mask;
        let yhdr = y * gain_per_pix;
        gain.push((yhdr + offset) / (y + offset));
    }
    let gmin = gain.iter().copied().fold(f64::INFINITY, f64::min);
    let gmax = gain.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let min_boost = gmin.min(1.0).clamp(0.25, 1.0);
    let max_boost_actual = gmax.max(1.0);
    let map_min = min_boost.log2();
    let map_max = max_boost_actual.log2();
    let range = (map_max - map_min).max(1e-6);

    // 全分辨率 ratio → 8-bit
    let gm_full: Vec<u8> = gain
        .iter()
        .map(|g| {
            let log_rec = (g.log2() - map_min) / range;
            let rec = log_rec.clamp(0.0, 1.0);
            (rec * 255.0).round().clamp(0.0, 255.0) as u8
        })
        .collect();

    let gm_w = (w / 4).max(1);
    let gm_h = (h / 4).max(1);
    // 双线性 decimation（旧）—— 缺乏低通预滤波
    downscale_bilinear(&gm_full, w, h, gm_w, gm_h)
}

/// 模拟"第一步 + box 修正"：全分辨率逐像素 ratio + box-average decimation。
/// （第一步改动后的中间状态，仅用于对比）
fn step1_fullres_then_box(rgba: &[u8], w: usize, h: usize, settings: &Settings) -> Vec<u8> {
    let hdr_intensity = settings.gain_ev();
    let gamma = settings.gamma;
    let user_max_boost = 2.0f64.powf(hdr_intensity).clamp(1.0, 64.0);
    let white_nits = settings.white_nits;
    let peak_cap = (settings.peak_nits / white_nits).max(1.0);
    let max_boost = user_max_boost.min(peak_cap);
    let highlight_start = 0.5f64;
    let offset = 1.0 / 64.0;

    let n = w * h;
    let mut gain = Vec::with_capacity(n);
    for i in 0..n {
        let base = i * 4;
        let r = srgb_to_linear(rgba[base] as f64 / 255.0);
        let g = srgb_to_linear(rgba[base + 1] as f64 / 255.0);
        let b = srgb_to_linear(rgba[base + 2] as f64 / 255.0);
        let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        let mask = ((y - highlight_start) / (1.0 - highlight_start))
            .clamp(0.0, 1.0)
            .powf(gamma);
        let gain_per_pix = 1.0 + (max_boost - 1.0) * mask;
        let yhdr = y * gain_per_pix;
        gain.push((yhdr + offset) / (y + offset));
    }
    let gmin = gain.iter().copied().fold(f64::INFINITY, f64::min);
    let gmax = gain.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let min_boost = gmin.min(1.0).clamp(0.25, 1.0);
    let max_boost_actual = gmax.max(1.0);
    let map_min = min_boost.log2();
    let map_max = max_boost_actual.log2();
    let range = (map_max - map_min).max(1e-6);

    let gm_full: Vec<u8> = gain
        .iter()
        .map(|g| {
            let log_rec = (g.log2() - map_min) / range;
            let rec = log_rec.clamp(0.0, 1.0);
            (rec * 255.0).round().clamp(0.0, 255.0) as u8
        })
        .collect();

    let gm_w = (w / 4).max(1);
    let gm_h = (h / 4).max(1);
    // box 平均（下采样）—— 第一步改动后的状态
    downscale_area_average_box(&gm_full, w, h, gm_w, gm_h)
}

fn main() {
    let mut args = std::env::args().skip(1);
    let input = args.next().expect("用法: dump_gainmap_compare <input.png> <out_dir>");
    let out_dir = args.next().expect("用法: dump_gainmap_compare <input.png> <out_dir>");
    let out_dir = PathBuf::from(out_dir);
    std::fs::create_dir_all(&out_dir).expect("创建输出目录失败");

    let img = image::open(&input).expect("无法打开输入图像").to_rgba8();
    let (w, h) = image::GenericImageView::dimensions(&img);
    println!("输入: {input}");
    println!("尺寸: {w}x{h}");

    let rgba: Vec<u8> = img.into_raw();

    let settings = Settings::default();

    // 第一步（历史·最旧）：全分辨率逐像素 + 双线性 decimation（无低通，aliasing）
    let gm_step1_bilinear = step1_fullres_then_bilinear(&rgba, w as usize, h as usize, &settings);
    let gm_w = (w as usize / 4).max(1);
    let gm_h = (h as usize / 4).max(1);
    assert_eq!(gm_step1_bilinear.len(), gm_w * gm_h);
    let path_step1_bilinear = out_dir.join("gainmap_step1_fullres_bilinear.png");
    save_gray_png(&path_step1_bilinear, &gm_step1_bilinear, gm_w as u32, gm_h as u32);
    println!("step1-bilinear (fullres+bilinear): {}", path_step1_bilinear.display());

    // 第一步（修正后）：全分辨率逐像素 + box decimation（有低通，但 ratio 已是高频）
    let gm_step1_box = step1_fullres_then_box(&rgba, w as usize, h as usize, &settings);
    let path_step1_box = out_dir.join("gainmap_step1_fullres_box.png");
    save_gray_png(&path_step1_box, &gm_step1_box, gm_w as u32, gm_h as u32);
    println!("step1-box      (fullres+box):      {}", path_step1_box.display());

    // 第二步（已完成改动：box 下采样主图 + 低分辨率硬阈值 mask）
    // 自实现，不复用 compute_gain_map（compute_gain_map 现在是 step3 实现，带 mask blur）。
    let hdr_intensity = settings.gain_ev();
    let gamma = settings.gamma;
    let user_max_boost = 2.0f64.powf(hdr_intensity).clamp(1.0, 64.0);
    let white_nits = settings.white_nits;
    let peak_cap = (settings.peak_nits / white_nits).max(1.0);
    let max_boost = user_max_boost.min(peak_cap);
    let offset = 1.0f64 / 64.0;
    let low_rgba = hdrconv::ultra_hdr::downscale_area_average_box_rgba(
        &rgba, w as usize, h as usize, gm_w, gm_h,
    );
    let mask_hard: Vec<f64> = (0..gm_w * gm_h)
        .map(|i| {
            let base = i * 4;
            let r = srgb_to_linear(low_rgba[base] as f64 / 255.0);
            let g = srgb_to_linear(low_rgba[base + 1] as f64 / 255.0);
            let b = srgb_to_linear(low_rgba[base + 2] as f64 / 255.0);
            let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
            ((y - 0.5) / 0.5).clamp(0.0, 1.0).powf(gamma)
        })
        .collect();
    let gain_step2: Vec<f64> = (0..gm_w * gm_h)
        .map(|i| {
            let base = i * 4;
            let r = srgb_to_linear(low_rgba[base] as f64 / 255.0);
            let g = srgb_to_linear(low_rgba[base + 1] as f64 / 255.0);
            let b = srgb_to_linear(low_rgba[base + 2] as f64 / 255.0);
            let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
            let m = mask_hard[i].clamp(0.0, 1.0);
            let gain_per_pix = 1.0 + (max_boost - 1.0) * m;
            let yhdr = y * gain_per_pix;
            (yhdr + offset) / (y + offset)
        })
        .collect();
    let gmin2 = gain_step2.iter().copied().fold(f64::INFINITY, f64::min);
    let gmax2 = gain_step2.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let min_b2 = gmin2.min(1.0).clamp(0.25, 1.0);
    let max_b2 = gmax2.max(1.0);
    let map_min2 = min_b2.log2();
    let map_max2 = max_b2.log2();
    let range2 = (map_max2 - map_min2).max(1e-6);
    let gm_step2: Vec<u8> = gain_step2
        .iter()
        .map(|g| {
            let r = (g.log2() - map_min2) / range2;
            (r.clamp(0.0, 1.0) * 255.0).round().clamp(0.0, 255.0) as u8
        })
        .collect();
    let path_step2 = out_dir.join("gainmap_step2_lowres.png");
    save_gray_png(&path_step2, &gm_step2, gm_w as u32, gm_h as u32);
    println!("step2          (lowres hard thr):   {}", path_step2.display());

    // 第三步（当前主链路）：compute_gain_map 内部已用 gaussian_blur_33 把硬阈值变软
    let (gm_step3, _meta) = compute_gain_map(&rgba, w as usize, h as usize, &settings);
    assert_eq!(gm_step3.len(), gm_w * gm_h);
    let path_step3 = out_dir.join("gainmap_step3_lowres_maskblur.png");
    save_gray_png(&path_step3, &gm_step3, gm_w as u32, gm_h as u32);
    println!("step3          (lowres soft thr):   {}", path_step3.display());

    // 差异：step1-bilinear vs step3（关键链：旧 vs 新）
    let diff_step1b_vs_step3: Vec<u8> = gm_step1_bilinear
        .iter()
        .zip(gm_step3.iter())
        .map(|(a, b)| a.abs_diff(*b))
        .collect();
    let path_diff_b3 = out_dir.join("gainmap_step1_bilinear_vs_step3_diff.png");
    save_gray_png(&path_diff_b3, &diff_step1b_vs_step3, gm_w as u32, gm_h as u32);
    let max_diff_b3 = diff_step1b_vs_step3.iter().copied().max().unwrap_or(0);
    let avg_diff_b3 =
        diff_step1b_vs_step3.iter().map(|x| *x as u32).sum::<u32>() as f64 / diff_step1b_vs_step3.len() as f64;
    println!(
        "diff step1-bilinear vs step3: max={max_diff_b3}, mean={avg_diff_b3:.2}  {}",
        path_diff_b3.display()
    );

    // 差异：step2 vs step3（关键：本步骤 vs 上一步改善多少）
    let diff_step2_vs_step3: Vec<u8> = gm_step2
        .iter()
        .zip(gm_step3.iter())
        .map(|(a, b)| a.abs_diff(*b))
        .collect();
    let path_diff_23 = out_dir.join("gainmap_step2_vs_step3_diff.png");
    save_gray_png(&path_diff_23, &diff_step2_vs_step3, gm_w as u32, gm_h as u32);
    let max_diff_23 = diff_step2_vs_step3.iter().copied().max().unwrap_or(0);
    let avg_diff_23 =
        diff_step2_vs_step3.iter().map(|x| *x as u32).sum::<u32>() as f64 / diff_step2_vs_step3.len() as f64;
    println!(
        "diff step2       vs step3:     max={max_diff_23}, mean={avg_diff_23:.2}  {}",
        path_diff_23.display()
    );

    // 保留 step2-only 差值（与旧报告对齐）
    let diff_step1b_vs_step2: Vec<u8> = gm_step1_bilinear
        .iter()
        .zip(gm_step2.iter())
        .map(|(a, b)| a.abs_diff(*b))
        .collect();
    let _ = save_gray_png(
        &out_dir.join("gainmap_step1_bilinear_vs_step2_diff.png"),
        &diff_step1b_vs_step2,
        gm_w as u32,
        gm_h as u32,
    );
    let diff_step1_box_vs_step2: Vec<u8> = gm_step1_box
        .iter()
        .zip(gm_step2.iter())
        .map(|(a, b)| a.abs_diff(*b))
        .collect();
    let _ = save_gray_png(
        &out_dir.join("gainmap_step1_box_vs_step2_diff.png"),
        &diff_step1_box_vs_step2,
        gm_w as u32,
        gm_h as u32,
    );

    // 找一个差异最剧烈的位置打印附近剖面（基于 step1-bilinear vs step3，最终对比）
    let mut worst_y = 0usize;
    let mut worst_sum = 0u64;
    for y in 0..gm_h {
        let s: u64 = (0..gm_w).map(|x| diff_step1b_vs_step3[y * gm_w + x] as u64).sum();
        if s > worst_sum {
            worst_sum = s;
            worst_y = y;
        }
    }
    println!(
        "\nstep1-bilinear vs step3 差异最剧烈行 y={worst_y} (sum={worst_sum})，前 60 列剖面："
    );
    print!("step1-bilinear: ");
    for x in 0..60 {
        print!("{:3} ", gm_step1_bilinear[worst_y * gm_w + x]);
    }
    println!();
    print!("step1-box      : ");
    for x in 0..60 {
        print!("{:3} ", gm_step1_box[worst_y * gm_w + x]);
    }
    println!();
    print!("step2          : ");
    for x in 0..60 {
        print!("{:3} ", gm_step2[worst_y * gm_w + x]);
    }
    println!();
    print!("step3          : ");
    for x in 0..60 {
        print!("{:3} ", gm_step3[worst_y * gm_w + x]);
    }
    println!();
    print!("diff(step2-step3): ");
    for x in 0..60 {
        print!("{:3} ", diff_step2_vs_step3[worst_y * gm_w + x]);
    }
    println!();
}

fn save_gray_png(path: &PathBuf, data: &[u8], w: u32, h: u32) {
    let buf: ImageBuffer<image::Luma<u8>, _> = ImageBuffer::from_raw(w, h, data.to_vec()).unwrap();
    buf.save(path).expect("写入 PNG 失败");
}

/// 旧 ratio 函数（与 compute_gain_map 在第一步时一致）。
fn srgb_to_linear(v: f64) -> f64 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}
