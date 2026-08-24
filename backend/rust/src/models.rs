//! ← Models.kt：ConversionSettings / RgbAdjustment 的 Rust 对应。
//!
//! 对照关系：
//! - `ConversionSettings.hdrIntensity * fineTuneBrightness` → `Settings::total_exposure()`
//!   （Electron 前端已移除「微调明暗」，改用峰值亮度统一控制：总曝光 = 峰值/白点 = 2^EV，
//!   与 Main.kt:454 `peak = (peakNits / whiteNits).coerceAtLeast(1.0)` 一致）
//! - `peakNits` / `whiteNits` 默认值注意区分：Kotlin 后端回退默认 1000/203，
//!   Electron UI 默认 574/203（本 CLI 采用 UI 默认）。

use std::fmt;

/// 输出格式；对应 Electron 前端 outputFormat（png / jpg / jpg_icc / ultra_hdr）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    /// 普通 HDR PNG（BT.2020 ICC，见 assets/2020_profile.icc 注入路径）
    Png,
    /// 普通 JPEG（Rec.2020/PQ 像素直出，无增益图）。CLI 的 "jpg" 已映射到 `UltraHdr`
    /// （Kotlin `/convert` 语义），本变体仅作 API 使用。
    Jpg,
    /// HDR JPEG：ICC 增益（Display-P3 + ICC），对应前端 "jpg_icc"
    JpgIcc,
    /// Ultra HDR JPEG：增益图 + MPF + GContainer/hdrgm XMP + ICC（Kotlin "jpg" 语义）
    UltraHdr,
}

impl OutputFormat {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.to_ascii_lowercase().as_str() {
            "png" | "hdr-png" => Ok(Self::Png),
            "jpg" | "jpeg" => Ok(Self::UltraHdr), // Kotlin /convert 的 "jpg" 即 Ultra HDR
            "jpg-icc" | "jpeg-icc" | "jpg_icc" => Ok(Self::JpgIcc),
            "ultra-hdr" | "ultrahdr" | "uhdr" => Ok(Self::UltraHdr),
            other => Err(format!(
                "未知输出格式: {other}（可选 png | jpg | jpg-icc | ultra-hdr）"
            )),
        }
    }

    /// 默认输出文件扩展名。
    pub fn ext(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpg | Self::JpgIcc | Self::UltraHdr => "jpg",
        }
    }
}

impl fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Png => "png",
            Self::Jpg => "jpg",
            Self::JpgIcc => "jpg-icc",
            Self::UltraHdr => "ultra-hdr",
        };
        f.write_str(s)
    }
}

/// ← Models.kt `RgbAdjustment`（默认 red=0.96, green=1.0, blue=1.0）。
#[derive(Debug, Clone, Copy)]
pub struct RgbAdjustment {
    pub red: f64,
    pub green: f64,
    pub blue: f64,
}

impl Default for RgbAdjustment {
    fn default() -> Self {
        Self {
            red: 0.96,
            green: 1.0,
            blue: 1.0,
        }
    }
}

/// ← Models.kt `ConversionSettings` 的 Rust 对应（保留 Kotlin 字段语义）。
#[derive(Debug, Clone)]
pub struct Settings {
    /// 峰值亮度（尼特）：高光上限。默认 574 = 203×2^1.5EV（Electron UI 默认）。
    pub peak_nits: f64,
    /// 白点（SDR 参考白，尼特，BT.2408）。默认 203。
    pub white_nits: f64,
    /// 伽马。默认 0.9。
    pub gamma: f64,
    /// RGB 通道调整（仅「直接转」生效）。
    pub rgb: RgbAdjustment,
    /// JPEG 质量 0..1（默认 1.0=100%），仅 jpg 类输出生效。
    pub quality: f64,
    /// 主图用原始 sRGB 像素 + sRGB ICC（实验，对应 Kotlin primarySrgb）。
    pub primary_srgb: bool,
    /// ICC 配置文件路径（None = 自动探测 assets/2020_profile.icc）。
    /// 注意：**不属于** Kotlin ConversionSettings —— 对应后端起服务时加载的 ICC
    /// （Kotlin `resolveIccProfilePath`，png / jpg_icc 均注入 2020_profile.icc）。
    pub icc_path: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            peak_nits: 574.0,
            white_nits: 203.0,
            gamma: 0.9,
            rgb: RgbAdjustment::default(),
            quality: 1.0,
            primary_srgb: false,
            icc_path: None,
        }
    }
}

impl Settings {
    /// 总曝光 = 峰值/白点（2^EV）。对应 Kotlin `ConversionSettings.totalExposure`
    /// 与 Main.kt:454 的 `peak` 计算（coerceAtLeast(1.0) 防 <1）。
    pub fn total_exposure(&self) -> f64 {
        (self.peak_nits / self.white_nits).max(1.0)
    }

    /// 增益图 maxBoost = 2^EV = 峰值/白点（README：图片 Ultra HDR 增益图 maxBoost 随峰值联动）。
    pub fn max_boost(&self) -> f64 {
        self.total_exposure()
    }

    /// EV = log2(峰值/白点)，用于视频 MaxCLL/NPL/PAM 峰值换算。
    pub fn ev(&self) -> f64 {
        (self.peak_nits / self.white_nits).log2()
    }
}