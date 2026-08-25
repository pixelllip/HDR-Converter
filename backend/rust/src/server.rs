//! axum HTTP 服务：1:1 复刻 Kotlin 后端（backend/kotlin Main.kt）的端点契约。
//!
//! 目标：Electron 主进程把 spawn 目标从 `java -jar` 换成 `hdrconv serve` 即可无缝切换
//! （端口行格式 `HDR_BACKEND_PORT:<port>` 与 Kotlin 一致，main.js 直接可解析）。
//!
//! 端点（与 Kotlin 对齐）：
//!   GET  /health          → { status:"ok", message }（waitReady 依赖）
//!   GET  /progress        → { value, active, message }（单张转换进度轮询）
//!   GET  /status          → { method, threads, capacity, active, gpuName, message }
//!   POST /convert         → ConvertResponse { success, outputPath, outputFormat, message, detectedColorSpace }
//!   POST /cancel          → { ok:"true" }
//!   POST /preview         → PreviewResponse { dataUrl, width, height, aspectRatio }
//!   POST /estimate        → EstimateResponse { hdrIntensity, maxBoost, yP995, hlRatio, message }
//!   POST /video-frame     → 原始 PAM 字节（无 outputPath）或 { ok,width,height }（带 outputPath）
//!   POST /batch/convert   → BatchConvertResponse { results, successCount, failCount }
//!   GET  /batch/progress  → BatchProgressResponse { total,done,failed,current,message,running,statuses }
//!   POST /batch/cancel    → { ok:"true" }
//!
//! 与 Kotlin 差异（有意）：转换由 Rust 库函数完成（更快）；kubernetes/gpu 相关字段
//! 返回 CPU 语义；进度状态为尽力而为（Rust 转换很快，轮询窗口小）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;

use crate::models::{OutputFormat, RgbAdjustment, Settings};
use crate::{colorspace, convert, ultra_hdr};

// ============================================================
//  状态（对应 Kotlin ConversionProgress / SingleCancel / BatchProgress / BatchCancel）
// ============================================================

#[derive(Default)]
struct SingleProgress {
    value: f64,
    active: bool,
    message: String,
}

#[derive(Default)]
struct BatchState {
    total: usize,
    done: usize,
    failed: usize,
    current: String,
    message: String,
    running: bool,
    statuses: HashMap<String, String>,
}

#[derive(Default)]
struct ServerState {
    single: Mutex<SingleProgress>,
    batch: Mutex<BatchState>,
    single_cancel: AtomicBool,
    batch_cancel: Mutex<Vec<String>>,
}

type Shared = Arc<ServerState>;

// ============================================================
//  JSON 契约（← Models.kt 字段名逐一对齐）
// ============================================================

#[derive(Deserialize, Default, Clone)]
struct JsonSettings {
    #[serde(default, rename = "hdrIntensity")]
    hdr_intensity: Option<f64>,
    #[serde(default, rename = "fineTuneBrightness")]
    #[allow(dead_code)] // 契约字段：Kotlin 旧语义（微调明暗已移除，png/jpg 路径不再使用）
    fine_tune_brightness: Option<f64>,
    #[serde(default, rename = "gamma")]
    gamma: Option<f64>,
    #[serde(default, rename = "rgbAdjustment")]
    rgb_adjustment: Option<JsonRgb>,
    #[serde(default, rename = "outputFormat")]
    output_format: Option<String>,
    #[serde(default, rename = "quality")]
    quality: Option<f64>,
    #[serde(default, rename = "primarySrgb")]
    primary_srgb: bool,
    #[serde(default, rename = "whiteNits")]
    white_nits: Option<f64>,
    #[serde(default, rename = "peakNits")]
    peak_nits: Option<f64>,
}

#[derive(Deserialize, Default, Clone)]
struct JsonRgb {
    #[serde(default, rename = "red")]
    red: Option<f64>,
    #[serde(default, rename = "green")]
    green: Option<f64>,
    #[serde(default, rename = "blue")]
    blue: Option<f64>,
}

impl JsonSettings {
    /// → Settings。默认值与 Kotlin `ConversionSettings` 一致：
    /// peak=1000（**注意：非 CLI 的 574**）、white=203、gamma=0.9、hdrIntensity=1.18、
    /// rgb=0.96/1/1、quality=1.0、outputFormat=png。
    fn to_settings(&self) -> Settings {
        Settings {
            peak_nits: self.peak_nits.unwrap_or(1000.0),
            white_nits: self.white_nits.unwrap_or(203.0),
            gamma: self.gamma.unwrap_or(0.9),
            rgb: RgbAdjustment {
                red: self.rgb_adjustment.as_ref().and_then(|r| r.red).unwrap_or(0.96),
                green: self.rgb_adjustment.as_ref().and_then(|r| r.green).unwrap_or(1.0),
                blue: self.rgb_adjustment.as_ref().and_then(|r| r.blue).unwrap_or(1.0),
            },
            quality: self.quality.unwrap_or(1.0),
            primary_srgb: self.primary_srgb,
            // Kotlin 默认 hdrIntensity=1.18（增益图 EV）；CLI 的 None=峰值联动仅限 CLI
            hdr_intensity: Some(self.hdr_intensity.unwrap_or(1.18)),
            icc_path: None,
        }
    }

    fn output_format(&self) -> OutputFormat {
        self.output_format
            .as_deref()
            .and_then(|s| OutputFormat::parse(s).ok())
            .unwrap_or(OutputFormat::Png)
    }
}

#[derive(Deserialize)]
struct ConvertReq {
    #[serde(rename = "inputPath")]
    input_path: String,
    #[serde(default, rename = "outputPath")]
    output_path: Option<String>,
    #[serde(default)]
    settings: Option<JsonSettings>,
}

#[derive(Deserialize)]
struct PreviewReq {
    #[serde(rename = "inputPath")]
    input_path: String,
    #[serde(default)]
    settings: Option<JsonSettings>,
    #[serde(default)]
    mode: Option<String>,
}

#[derive(Deserialize)]
struct EstimateReq {
    #[serde(rename = "inputPath")]
    input_path: String,
}

#[derive(Deserialize)]
struct VideoFrameReq {
    #[serde(rename = "inputPath")]
    input_path: String,
    #[serde(default)]
    settings: Option<JsonSettings>,
    #[serde(default)]
    peak: Option<f64>,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default, rename = "outputPath")]
    output_path: Option<String>,
}

#[derive(Deserialize, Clone)]
struct BatchJob {
    #[serde(rename = "inputPath")]
    input_path: String,
    #[serde(default, rename = "outputPath")]
    output_path: Option<String>,
    #[serde(default)]
    settings: Option<JsonSettings>,
}

#[derive(Deserialize)]
struct BatchConvertReq {
    #[serde(default)]
    jobs: Vec<BatchJob>,
    #[serde(default, rename = "maxConcurrent")]
    max_concurrent: Option<usize>,
}

#[derive(Deserialize)]
struct BatchCancelReq {
    #[serde(default, rename = "inputPaths")]
    input_paths: Vec<String>,
}

// 响应（字段名与 Kotlin 完全一致——camelCase）
#[derive(Serialize)]
struct ConvertResp {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none", rename = "outputPath")]
    output_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "outputFormat")]
    output_format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "detectedColorSpace")]
    detected_color_space: Option<String>,
}

#[derive(Serialize)]
struct PreviewResp {
    #[serde(skip_serializing_if = "Option::is_none", rename = "dataUrl")]
    data_url: Option<String>,
    width: u32,
    height: u32,
    #[serde(skip_serializing_if = "Option::is_none", rename = "aspectRatio")]
    aspect_ratio: Option<f64>,
}

#[derive(Serialize)]
struct EstimateResp {
    #[serde(rename = "hdrIntensity")]
    hdr_intensity: f64,
    #[serde(rename = "maxBoost")]
    max_boost: f64,
    #[serde(rename = "yP995")]
    y_p995: f64,
    #[serde(rename = "hlRatio")]
    hl_ratio: f64,
    message: String,
}

#[derive(Serialize)]
struct VideoFrameResp {
    ok: bool,
    width: i32,
    height: i32,
}

#[derive(Serialize)]
struct BatchJobResult {
    #[serde(rename = "inputPath")]
    input_path: String,
    #[serde(skip_serializing_if = "Option::is_none", rename = "outputPath")]
    output_path: Option<String>,
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

#[derive(Serialize)]
struct BatchConvertResp {
    results: Vec<BatchJobResult>,
    #[serde(rename = "successCount")]
    success_count: usize,
    #[serde(rename = "failCount")]
    fail_count: usize,
}

#[derive(Serialize)]
struct BatchProgressResp {
    total: usize,
    done: usize,
    failed: usize,
    current: String,
    message: String,
    running: bool,
    statuses: HashMap<String, String>,
}

// ============================================================
//  处理器
// ============================================================

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok", "message": "HDR Converter Backend is running" }))
}

async fn progress(State(st): State<Shared>) -> Json<serde_json::Value> {
    let p = st.single.lock().unwrap();
    Json(serde_json::json!({
        "value": p.value,
        "active": p.active,
        "message": p.message,
    }))
}

async fn status(State(_st): State<Shared>) -> Json<serde_json::Value> {
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    let capacity = (threads / 2 + 1).max(1);
    Json(serde_json::json!({
        "method": "cpu",
        "threads": threads.to_string(),
        "capacity": capacity.to_string(),
        "active": 0,
        "gpuName": "",
        "message": format!("CPU 多线程（{threads} 核）"),
    }))
}

async fn convert(State(st): State<Shared>, Json(req): Json<ConvertReq>) -> Response {
    let input = PathBuf::from(&req.input_path);
    if st.single_cancel.swap(false, Ordering::SeqCst) {
        return Json(ConvertResp {
            success: false,
            output_path: req.output_path.clone(),
            output_format: req.settings.as_ref().map(|s| s.output_format().to_string()),
            message: Some("已取消".into()),
            detected_color_space: None,
        })
        .into_response();
    }
    let settings = req
        .settings
        .as_ref()
        .map(|s| s.to_settings())
        .unwrap_or_else(|| JsonSettings::default().to_settings());
    let format = req
        .settings
        .as_ref()
        .map(|s| s.output_format())
        .unwrap_or(OutputFormat::Png);
    let ext = if format == OutputFormat::Png { ".png" } else { ".jpg" };
    let out = req
        .output_path
        .clone()
        .unwrap_or_else(|| format!("output{ext}"));
    let out = if out.to_lowercase().ends_with(&format!("{ext}")) { out } else { format!("{out}{ext}") };

    let out_res = out.clone();
    let st_work = st.clone();
    let outer = match tokio::task::spawn_blocking(move || -> anyhow::Result<(String, String, String)> {
        st_work.single.lock().unwrap().active = true;
        st_work.single.lock().unwrap().message = "读取图片".into();
        let detected = colorspace::detect(&input);
        let img = convert::read_image_rgba(&input)?;
        st_work.single.lock().unwrap().message = "开始编码".into();
        let bytes = crate::encode_image_bytes(&img, &settings, format, Some(detected))?;
        std::fs::write(&out_res, bytes)?;
        Ok((out_res, format.to_string(), detected.to_string()))
    })
    .await
    {
        Ok(r) => r,
        Err(e) => return Json(ConvertResp {
            success: false,
            output_path: Some(out),
            output_format: Some(format.to_string()),
            message: Some(format!("转换失败: {e:#}")),
            detected_color_space: None,
        })
        .into_response(),
    };
    st.single.lock().unwrap().active = false;
    st.single.lock().unwrap().value = 1.0;
    match outer {
        Ok((path, fmt, cs)) => Json(ConvertResp {
            success: true,
            output_path: Some(path),
            output_format: Some(fmt),
            message: Some("转换完成，输出已保存".into()),
            detected_color_space: Some(cs),
        })
        .into_response(),
        Err(e) => Json(ConvertResp {
            success: false,
            output_path: Some(out),
            output_format: Some(format.to_string()),
            message: Some(format!("{e:#}")),
            detected_color_space: None,
        })
        .into_response(),
    }
}

async fn cancel(State(st): State<Shared>) -> Json<serde_json::Value> {
    st.single_cancel.store(true, Ordering::SeqCst);
    Json(serde_json::json!({ "ok": "true" }))
}

async fn preview(State(st): State<Shared>, Json(req): Json<PreviewReq>) -> Response {
    let input = PathBuf::from(&req.input_path);
    let settings = req.settings.as_ref().map(|s| s.to_settings()).unwrap_or_else(|| {
        JsonSettings::default().to_settings()
    });
    let mode = req.mode.clone().unwrap_or_default();
    let format = req.settings.as_ref().map(|s| s.output_format()).unwrap_or(OutputFormat::Png);

    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<(Vec<u8>, u32, u32, String)> {
        let img = convert::read_image_for_preview(&input, 0.5)?;
        let (w, h) = (img.width, img.height);
        let mime;
        let bytes = if mode == "videoDirect" {
            // 视频直接转预览：与视频输出一致的 Rec.2020/PQ（曝光=峰值）
            mime = "image/jpeg".to_string();
            let white = settings.white_nits;
            let peak = (settings.peak_nits / white).max(1.0);
            let rgba = ultra_hdr::video_direct_preview_rgba(
                &img.pixels, img.width, img.height, &settings, peak, white,
            );
            let rgb_img = convert::ImageData { pixels: rgba, width: w, height: h };
            let jpeg = convert::encode_jpeg_bytes(&rgb_img, settings.quality.clamp(0.1, 1.0))?;
            crate::icc::inject_icc_into_jpeg(&jpeg, &crate::resolve_icc(&settings)?)?
        } else {
            let bytes = crate::encode_image_bytes(&img, &settings, format, None)?;
            mime = if format == OutputFormat::Png { "image/png" } else { "image/jpeg" }.to_string();
            bytes
        };
        Ok((bytes, w, h, mime))
    })
    .await;
    st.single.lock().unwrap().active = false;

    match result {
        Ok(Ok((bytes, w, h, mime))) => {
            let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
            Json(PreviewResp {
                data_url: Some(format!("data:{mime};base64,{b64}")),
                width: w,
                height: h,
                aspect_ratio: Some(w as f64 / h.max(1) as f64),
            })
            .into_response()
        }
        _ => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": "预览失败" })),
        )
            .into_response(),
    }
}

async fn estimate(State(_st): State<Shared>, Json(req): Json<EstimateReq>) -> Response {
    let input = PathBuf::from(&req.input_path);
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<ultra_hdr::IntensityEstimate> {
        let img = convert::read_image_for_preview(&input, 0.25)?;
        Ok(ultra_hdr::estimate_hdr_intensity(
            &img.pixels,
            img.width as usize,
            img.height as usize,
        ))
    })
    .await;
    match result {
        Ok(Ok(e)) => Json(EstimateResp {
            hdr_intensity: e.hdr_intensity,
            max_boost: e.max_boost,
            y_p995: e.y_p995,
            hl_ratio: e.hl_ratio,
            message: format!(
                "已自动估算 HDR 强度 {:.2} EV（maxBoost ×{:.1}）",
                e.hdr_intensity, e.max_boost
            ),
        })
        .into_response(),
        _ => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": "估算失败" })),
        )
            .into_response(),
    }
}

async fn video_frame(State(_st): State<Shared>, Json(req): Json<VideoFrameReq>) -> Response {
    let input = PathBuf::from(&req.input_path);
    let settings = req.settings.as_ref().map(|s| s.to_settings()).unwrap_or_else(|| {
        JsonSettings::default().to_settings()
    });
    let peak = req.peak.unwrap_or(8.0);
    let mode = req.mode.clone().unwrap_or_else(|| "gainmap".into());
    let output_path = req.output_path.clone();

    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<u8>> {
        let img = convert::read_image_rgba(&input)?;
        // Kotlin：gainmap 用 reconstructLinearHdrFrame（EV=settings.hdrIntensity），
        // transform 用 reconstructLinearHdrTransform（曝光=peak，无自动伽马）
        if mode == "transform" {
            ultra_hdr::reconstruct_linear_hdr_transform(
                &img.pixels, img.width, img.height, &settings, peak,
            )
        } else {
            ultra_hdr::reconstruct_linear_hdr_frame(
                &img.pixels,
                img.width,
                img.height,
                &settings,
                peak,
                settings.gain_ev(),
            )
        }
    })
    .await;

    match result {
        Ok(Ok(pam)) => {
            if let Some(out_path) = output_path {
                // 兼容旧协议：直写 PAM 文件并回 JSON
                let p = Path::new(&out_path);
                if let Some(parent) = p.parent() {
                    std::fs::create_dir_all(parent).ok();
                }
                if std::fs::write(p, &pam).is_ok() {
                    Json(VideoFrameResp { ok: true, width: -1, height: -1 }).into_response()
                } else {
                    (StatusCode::INTERNAL_SERVER_ERROR, "写入失败").into_response()
                }
            } else {
                // 默认：原始 PAM 字节（application/octet-stream）
                ([(axum::http::header::CONTENT_TYPE, "application/octet-stream")], pam)
                    .into_response()
            }
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("逐帧重建失败: {e:#}")).into_response(),
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "逐帧重建失败").into_response(),
    }
}

async fn batch_convert(State(st): State<Shared>, Json(req): Json<BatchConvertReq>) -> Response {
    let jobs = req.jobs;
    let total = jobs.len();
    *st.batch.lock().unwrap() = BatchState {
        total,
        running: !jobs.is_empty(),
        ..Default::default()
    };
    if jobs.is_empty() {
        return Json(BatchConvertResp { results: vec![], success_count: 0, fail_count: 0 }).into_response();
    }

    let cancel_list = st.batch_cancel.lock().unwrap().clone();
    // 有界并发（对齐 Kotlin：默认 核心数/2+1，可 maxConcurrent 覆盖）
    let concurrency = req
        .max_concurrent
        .unwrap_or_else(|| {
            let c = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
            (c / 2 + 1).max(1)
        })
        .max(1);
    let sem = Arc::new(tokio::sync::Semaphore::new(concurrency));

    let mut handles = Vec::with_capacity(total);
    for job in jobs {
        let st = st.clone();
        let cancel_list = cancel_list.clone();
        let sem = sem.clone();
        handles.push(tokio::spawn(async move {
            let _permit = sem.acquire().await.unwrap();
            if cancel_list.iter().any(|p| p == &job.input_path) {
                return BatchJobResult {
                    input_path: job.input_path.clone(),
                    output_path: job.output_path.clone(),
                    success: false,
                    message: Some("已取消".into()),
                };
            }
            {
                let mut b = st.batch.lock().unwrap();
                b.current = job.input_path.clone();
                b.statuses.insert(job.input_path.clone(), "running".into());
            }
            // 内联转换（与 /convert 相同的 settings/输出名逻辑）
            let input = PathBuf::from(&job.input_path);
            let settings = job
                .settings
                .as_ref()
                .map(|s| s.to_settings())
                .unwrap_or_else(|| JsonSettings::default().to_settings());
            let format = job
                .settings
                .as_ref()
                .map(|s| s.output_format())
                .unwrap_or(OutputFormat::Jpg);
            let ext = if format == OutputFormat::Png { ".png" } else { ".jpg" };
            let out = job.output_path.clone().unwrap_or_else(|| format!("output{ext}"));
            let out = if out.to_lowercase().ends_with(ext) { out } else { format!("{out}{ext}") };
            let out_for = out.clone();
            let label = job.input_path.clone();
            let r = tokio::task::spawn_blocking(move || -> anyhow::Result<String> {
                let detected = colorspace::detect(&input);
                let img = convert::read_image_rgba(&input)?;
                let bytes = crate::encode_image_bytes(&img, &settings, format, Some(detected))?;
                std::fs::write(&out_for, bytes)?;
                Ok(out_for)
            })
            .await;
            let res = match r {
                Ok(Ok(p)) => BatchJobResult {
                    input_path: label,
                    output_path: Some(p),
                    success: true,
                    message: Some("转换完成，输出已保存".into()),
                },
                Ok(Err(e)) => BatchJobResult {
                    input_path: label,
                    output_path: Some(out),
                    success: false,
                    message: Some(format!("{e:#}")),
                },
                Err(e) => BatchJobResult {
                    input_path: label,
                    output_path: Some(out),
                    success: false,
                    message: Some(format!("任务失败: {e}")),
                },
            };
            {
                let mut b = st.batch.lock().unwrap();
                if res.success {
                    b.done += 1;
                } else {
                    b.failed += 1;
                }
                b.statuses
                    .insert(res.input_path.clone(), if res.success { "done".into() } else { "failed".into() });
            }
            res
        }));
    }
    let mut results = Vec::with_capacity(handles.len());
    for h in handles {
        if let Ok(r) = h.await {
            results.push(r);
        }
    }
    st.batch.lock().unwrap().running = false;
    let success_count = results.iter().filter(|r| r.success).count();
    let fail_count = results.len() - success_count;
    Json(BatchConvertResp { results, success_count, fail_count }).into_response()
}

async fn batch_cancel(State(st): State<Shared>, Json(req): Json<BatchCancelReq>) -> Json<serde_json::Value> {
    *st.batch_cancel.lock().unwrap() = req.input_paths;
    Json(serde_json::json!({ "ok": "true" }))
}

async fn batch_progress(State(st): State<Shared>) -> Json<BatchProgressResp> {
    let b = st.batch.lock().unwrap();
    Json(BatchProgressResp {
        total: b.total,
        done: b.done,
        failed: b.failed,
        current: b.current.clone(),
        message: b.message.clone(),
        running: b.running,
        statuses: b.statuses.clone(),
    })
}

// ============================================================
//  入口
// ============================================================

/// 启动 HTTP 服务（端口 None → 自动选可用端口，与 Kotlin findAvailablePort 一致）。
pub async fn serve(host: &str, port: Option<u16>) -> anyhow::Result<()> {
    let state: Shared = Arc::new(ServerState::default());

    let app = Router::new()
        .route("/health", get(health))
        .route("/progress", get(progress))
        .route("/status", get(status))
        .route("/batch/progress", get(batch_progress))
        .route("/convert", post(convert))
        .route("/cancel", post(cancel))
        .route("/preview", post(preview))
        .route("/estimate", post(estimate))
        .route("/video-frame", post(video_frame))
        .route("/batch/convert", post(batch_convert))
        .route("/batch/cancel", post(batch_cancel))
        .with_state(state);

    let listener = if let Some(p) = port {
        let addr = format!("{host}:{p}");
        TcpListener::bind(&addr).await.with_context_anyhow(&addr)?
    } else {
        TcpListener::bind(format!("{host}:0")).await.with_context_anyhow(host)?
    };
    let actual = listener.local_addr()?.port();
    // 与 Kotlin 相同的端口行格式（main.js / backend_test_util 用正则解析）
    println!("HDR_BACKEND_PORT:{actual}");
    axum::serve(listener, app).await?;
    Ok(())
}

trait WithContextAnyhow<T> {
    fn with_context_anyhow(self, what: &str) -> anyhow::Result<T>;
}

impl<T, E: std::fmt::Display> WithContextAnyhow<T> for Result<T, E> {
    fn with_context_anyhow(self, what: &str) -> anyhow::Result<T> {
        self.map_err(|e| anyhow::anyhow!("监听 {what} 失败: {e}"))
    }
}