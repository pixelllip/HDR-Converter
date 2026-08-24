//! ← HdrGpuJni.kt + backend/cuda（hdr_gpu_jni.cu 内核）：GPU 加速（可选，feature "gpu"）。
//!
//! ## 接入（2026-08 已完成 CUDA 侧）
//! 新增 `backend/cuda/jni/hdr_gpu_ffi.cu`（include hdr_gpu_jni.cu 全部内核）导出纯 C ABI：
//! 构建 `jni/build_ffi.bat`（nvcc，需 CUDA Toolkit + JDK jni.h + VS）→ `backend/cuda/hdr_gpu_ffi.dll`。
//! 本模块经 libloading 加载该 DLL（`hdr_ffi_*`），覆盖 6 个加速点：
//!   apply_hdr_rec2020_pq（png/jpg_icc 像素链路）、apply_hdr_transform（legacy）、
//!   srgb_to_p3 + compute_gainmap（Ultra HDR 主图/增益图）、reconstruct_gainmap16/transform16（视频逐帧）。
//!
//! ## 启用与精度
//! - 默认构建（无 feature）→ CPU；`cargo build --features gpu` 编译绑定；
//!   运行时需 DLL 可加载 **且** 环境变量 `HDRCONV_GPU=1`（显式选择，避免破坏 CPU 基准对齐）。
//! - GPU 内核为 **float32**（Kotlin CPU 为 float64）→ 输出可能与 CPU 差 ±1（8-bit）或 ±几十（16-bit）；
//!   保持默认 CPU 走逐位对齐契约，GPU 作为速度优先选项（见 tests 的 GPU==CPU 对照测量）。

#[cfg(feature = "gpu")]
use std::ffi::c_int;
#[cfg(feature = "gpu")]
use std::sync::OnceLock;

use crate::convert::ImageData;
use crate::models::Settings;

/// C ABI 契约（backend/cuda/jni/hdr_gpu_ffi.cu，全部 `extern "C"`、`__declspec(dllexport)`）。
///
/// ```c
/// int   hdr_ffi_init(int backend);
/// void  hdr_ffi_cleanup();
/// const char* hdr_ffi_error();
/// int   hdr_ffi_backend();
/// int   hdr_ffi_srgb_to_p3(const unsigned char* rgba, int w, int h, unsigned char* out);
/// int   hdr_ffi_compute_gainmap(const unsigned char* rgba, int w, int h,
///                               double hdrIntensity, double gamma, unsigned char* gm8, double* outMinMax);
/// int   hdr_ffi_apply_hdr_transform(const unsigned char* rgba, int w, int h,
///                                   double totalExposure, double gamma,
///                                   double rAdj, double gAdj, double bAdj, unsigned char* out);
/// int   hdr_ffi_apply_hdr_rec2020_pq(const unsigned char* rgba, int w, int h,
///                                    double exposure, double gamma,
///                                    double rAdj, double gAdj, double bAdj, double whiteNits, unsigned char* out);
/// int   hdr_ffi_reconstruct_gainmap16(const unsigned char* rgba, int w, int h,
///                                     double hdrIntensity, double gamma, double peak, unsigned char* out);
/// int   hdr_ffi_reconstruct_transform16(const unsigned char* rgba, int w, int h,
///                                       double exposure, double gamma,
///                                       double rAdj, double gAdj, double bAdj, double peak, unsigned char* out);
/// ```

#[cfg(feature = "gpu")]
pub mod bindings {
    use libloading::Library;
    use std::ffi::{c_char, c_int, CStr};

    type InitFn = unsafe extern "C" fn(c_int) -> c_int;
    type CleanupFn = unsafe extern "C" fn();
    type ErrorFn = unsafe extern "C" fn() -> *const c_char;
    type BackendFn = unsafe extern "C" fn() -> c_int;
    type SrgbToP3Fn = unsafe extern "C" fn(*const u8, c_int, c_int, *mut u8) -> c_int;
    type ComputeGainMapFn = unsafe extern "C" fn(
        *const u8,
        c_int,
        c_int,
        f64,
        f64,
        *mut u8,
        *mut f64,
    ) -> c_int;
    type ApplyTransformFn = unsafe extern "C" fn(
        *const u8,
        c_int,
        c_int,
        f64,
        f64,
        f64,
        f64,
        f64,
        *mut u8,
    ) -> c_int;
    type ApplyRec2020PqFn = unsafe extern "C" fn(
        *const u8,
        c_int,
        c_int,
        f64,
        f64,
        f64,
        f64,
        f64,
        f64,
        *mut u8,
    ) -> c_int;
    type Reconstruct16Fn = unsafe extern "C" fn(
        *const u8,
        c_int,
        c_int,
        f64,
        f64,
        f64,
        *mut u8,
    ) -> c_int;
    type Reconstruct16FullFn = unsafe extern "C" fn(
        *const u8,
        c_int,
        c_int,
        f64,
        f64,
        f64,
        f64,
        f64,
        f64,
        *mut u8,
    ) -> c_int;
    // 异步帧管线（视频逐帧；pinned 双缓冲 + 每槽 stream）
    type FramePrepareFn = unsafe extern "C" fn(c_int, c_int, c_int) -> c_int;
    type FrameSubmitFn = unsafe extern "C" fn(c_int, *const u8, c_int, *const f64, c_int) -> c_int;
    type FrameWaitFn = unsafe extern "C" fn(c_int, *mut u8) -> c_int;
    type FrameReleaseFn = unsafe extern "C" fn(c_int);

    /// GPU 句柄（进程内单例）。存裸函数指针（解引用自 Symbol），`_lib` 保持 DLL 存活。
    pub struct Gpu {
        _lib: Library,
        #[allow(dead_code)]
        init: InitFn,
        cleanup: CleanupFn,
        error: ErrorFn,
        backend_fn: BackendFn,
        srgb_to_p3: SrgbToP3Fn,
        compute_gainmap: ComputeGainMapFn,
        #[allow(dead_code)] // legacy applyHdrTransform（sRGB 输出），当前管线未用
        apply_transform: ApplyTransformFn,
        apply_rec2020_pq: ApplyRec2020PqFn,
        reconstruct_gainmap16: Reconstruct16Fn,
        reconstruct_transform16: Reconstruct16FullFn,
        frame_prepare: FramePrepareFn,
        frame_submit: FrameSubmitFn,
        frame_wait: FrameWaitFn,
        frame_release_fn: FrameReleaseFn,
    }

    unsafe impl Send for Gpu {}
    unsafe impl Sync for Gpu {}

    fn get<T: Copy>(lib: &Library, name: &[u8]) -> Option<T> {
        unsafe { lib.get::<T>(name).ok().map(|s| *s) }
    }

    impl Gpu {
        pub fn try_load(dll_path: &str) -> Option<Self> {
            let lib = unsafe { Library::new(dll_path) }.ok()?;
            let init = get::<InitFn>(&lib, b"hdr_ffi_init")?;
            if unsafe { init(0) } != 0 {
                return None;
            }
            // 先取全部函数指针，再 move lib（结构体字段求值有顺序，不能先借用后 move）
            let cleanup = get(&lib, b"hdr_ffi_cleanup")?;
            let error = get(&lib, b"hdr_ffi_error")?;
            let backend_fn = get(&lib, b"hdr_ffi_backend")?;
            let srgb_to_p3 = get(&lib, b"hdr_ffi_srgb_to_p3")?;
            let compute_gainmap = get(&lib, b"hdr_ffi_compute_gainmap")?;
            let apply_transform = get(&lib, b"hdr_ffi_apply_hdr_transform")?;
            let apply_rec2020_pq = get(&lib, b"hdr_ffi_apply_hdr_rec2020_pq")?;
            let reconstruct_gainmap16 = get(&lib, b"hdr_ffi_reconstruct_gainmap16")?;
            let reconstruct_transform16 = get(&lib, b"hdr_ffi_reconstruct_transform16")?;
            let frame_prepare = get(&lib, b"hdr_ffi_frame_prepare")?;
            let frame_submit = get(&lib, b"hdr_ffi_frame_submit")?;
            let frame_wait = get(&lib, b"hdr_ffi_frame_wait")?;
            let frame_release_fn = get(&lib, b"hdr_ffi_frame_release")?;
            Some(Gpu {
                _lib: lib,
                init,
                cleanup,
                error,
                backend_fn,
                srgb_to_p3,
                compute_gainmap,
                apply_transform,
                apply_rec2020_pq,
                reconstruct_gainmap16,
                reconstruct_transform16,
                frame_prepare,
                frame_submit,
                frame_wait,
                frame_release_fn,
            })
        }

        pub fn error_message(&self) -> String {
            unsafe {
                let p = (self.error)();
                if p.is_null() {
                    String::new()
                } else {
                    CStr::from_ptr(p).to_string_lossy().into_owned()
                }
            }
        }

        pub fn backend(&self) -> c_int {
            unsafe { (self.backend_fn)() }
        }

        pub fn srgb_to_p3(&self, rgba: &[u8], w: u32, h: u32, out: &mut [u8]) -> bool {
            if rgba.len() as u64 != w as u64 * h as u64 * 4 {
                return false;
            }
            unsafe {
                (self.srgb_to_p3)(rgba.as_ptr(), w as c_int, h as c_int, out.as_mut_ptr()) == 0
            }
        }

        pub fn compute_gainmap(
            &self,
            rgba: &[u8],
            w: u32,
            h: u32,
            hdr_intensity: f64,
            gamma: f64,
            gm8: &mut [u8],
            minmax: &mut [f64; 2],
        ) -> bool {
            if gm8.len() != w as usize * h as usize {
                return false;
            }
            unsafe {
                (self.compute_gainmap)(
                    rgba.as_ptr(),
                    w as c_int,
                    h as c_int,
                    hdr_intensity,
                    gamma,
                    gm8.as_mut_ptr(),
                    minmax.as_mut_ptr(),
                ) == 0
            }
        }

        pub fn apply_hdr_rec2020_pq(
            &self,
            rgba: &[u8],
            w: u32,
            h: u32,
            exposure: f64,
            gamma: f64,
            r_adj: f64,
            g_adj: f64,
            b_adj: f64,
            white_nits: f64,
            out: &mut [u8],
        ) -> bool {
            if out.len() != rgba.len() {
                return false;
            }
            unsafe {
                (self.apply_rec2020_pq)(
                    rgba.as_ptr(),
                    w as c_int,
                    h as c_int,
                    exposure,
                    gamma,
                    r_adj,
                    g_adj,
                    b_adj,
                    white_nits,
                    out.as_mut_ptr(),
                ) == 0
            }
        }

        pub fn reconstruct_gainmap16(
            &self,
            rgba: &[u8],
            w: u32,
            h: u32,
            hdr_intensity: f64,
            gamma: f64,
            peak: f64,
            out: &mut [u8],
        ) -> bool {
            if out.len() != w as usize * h as usize * 6 {
                return false;
            }
            unsafe {
                (self.reconstruct_gainmap16)(
                    rgba.as_ptr(),
                    w as c_int,
                    h as c_int,
                    hdr_intensity,
                    gamma,
                    peak,
                    out.as_mut_ptr(),
                ) == 0
            }
        }

        pub fn reconstruct_transform16(
            &self,
            rgba: &[u8],
            w: u32,
            h: u32,
            exposure: f64,
            gamma: f64,
            r_adj: f64,
            g_adj: f64,
            b_adj: f64,
            peak: f64,
            out: &mut [u8],
        ) -> bool {
            if out.len() != w as usize * h as usize * 6 {
                return false;
            }
            unsafe {
                (self.reconstruct_transform16)(
                    rgba.as_ptr(),
                    w as c_int,
                    h as c_int,
                    exposure,
                    gamma,
                    r_adj,
                    g_adj,
                    b_adj,
                    peak,
                    out.as_mut_ptr(),
                ) == 0
            }
        }

        // ---- 异步帧管线（视频逐帧） ----

        pub fn frame_prepare(&self, slot: c_int, w: c_int, h: c_int) -> bool {
            unsafe { (self.frame_prepare)(slot, w, h) == 0 }
        }

        /// mode: 0=gainmap16（params=[hdrIntensity,gamma,peak]）1=transform16（params=[exposure,gamma,rAdj,gAdj,bAdj,peak]）
        pub fn frame_submit(&self, slot: c_int, rgba: &[u8], mode: c_int, params: &[f64]) -> bool {
            unsafe {
                (self.frame_submit)(slot, rgba.as_ptr(), mode, params.as_ptr(), params.len() as c_int) == 0
            }
        }

        pub fn frame_wait(&self, slot: c_int, out: &mut [u8]) -> bool {
            unsafe { (self.frame_wait)(slot, out.as_mut_ptr()) == 0 }
        }

        pub fn frame_release(&self, slot: c_int) {
            unsafe { (self.frame_release_fn)(slot) }
        }
    }

    impl Drop for Gpu {
        fn drop(&mut self) {
            unsafe {
                (self.cleanup)();
            }
        }
    }
}

/// 异步帧管线模式（对齐 hdr_gpu_ffi.cu：0=gainmap16, 1=transform16）。
#[cfg(feature = "gpu")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameMode {
    Gainmap16,
    Transform16,
}

/// 帧管线（feature "gpu"）：pinned 双缓冲 + 每槽 stream 的异步泵。
///
/// 共享于视频 worker：worker 只调用 `submit`（满 4 槽时逐出最老的等待完成，
/// 经 channel 回传）；主线程在所有 worker 结束后调 `flush` + `close_tx` 收尾。
/// 并发安全：外部用 `Mutex<FramePump>` 保护。
#[cfg(feature = "gpu")]
pub struct FramePump {
    g: &'static bindings::Gpu,
    tx: Option<std::sync::mpsc::SyncSender<(usize, Vec<u8>)>>,
    pend: std::collections::VecDeque<(usize, usize)>, // (frame_idx, slot)
    slot_dims: [(u32, u32); 4],
    next_slot: usize,
    slots: usize,
}

#[cfg(feature = "gpu")]
const FRAME_SLOTS: usize = 4;

#[cfg(feature = "gpu")]
impl FramePump {
    pub fn try_new(tx: std::sync::mpsc::SyncSender<(usize, Vec<u8>)>) -> Option<Self> {
        let g = gpu()?;
        Some(FramePump {
            g,
            tx: Some(tx),
            pend: std::collections::VecDeque::new(),
            slot_dims: [(0, 0); FRAME_SLOTS],
            next_slot: 0,
            slots: FRAME_SLOTS,
        })
    }

    fn ensure_slot(&mut self, slot: usize, w: u32, h: u32) {
        if self.slot_dims[slot] != (w, h) {
            let _ = self.g.frame_prepare(slot as c_int, w as c_int, h as c_int);
            self.slot_dims[slot] = (w, h);
        }
    }

    /// 等待某槽完成 → 含 P7 头的完整 PAM（编码器 pam_pipe 可读）。失败返回 None（帧丢弃）。
    fn emit(&mut self, frame_idx: usize, slot: usize) -> Option<(usize, Vec<u8>)> {
        let (w, h) = self.slot_dims[slot];
        let mut px = vec![0u8; w as usize * h as usize * 6];
        if self.g.frame_wait(slot as c_int, &mut px) {
            // 与 CPU 路径一致：P7 头 + 16-bit 大端像素（← ultra_hdr::pam_with_pixels）
            Some((frame_idx, crate::ultra_hdr::pam_with_pixels(w, h, &px)))
        } else {
            eprintln!("[gpu] frame_wait 失败（槽 {slot}），帧 {frame_idx} 丢弃");
            None
        }
    }

    /// 提交一帧（异步）。满槽时逐出最老帧（等待完成），以 `Vec` 返回；
    /// 调用方应在**锁外**把这些帧经 channel 发给主线程（避免持锁阻塞）。
    pub fn submit(
        &mut self,
        frame_idx: usize,
        rgba: &[u8],
        w: u32,
        h: u32,
        mode: FrameMode,
        params: &[f64],
    ) -> anyhow::Result<Vec<(usize, Vec<u8>)>> {
        let mut done = Vec::new();
        while self.pend.len() >= self.slots {
            if let Some((f, s)) = self.pend.pop_front() {
                if let Some(d) = self.emit(f, s) {
                    done.push(d);
                }
            }
        }
        let slot = self.next_slot % self.slots;
        self.next_slot += 1;
        self.ensure_slot(slot, w, h);
        let mode_num = match mode {
            FrameMode::Gainmap16 => 0,
            FrameMode::Transform16 => 1,
        };
        if !self.g.frame_submit(slot as c_int, rgba, mode_num, params) {
            return Err(anyhow::anyhow!(
                "[gpu] frame_submit 失败：{}",
                self.g.error_message()
            ));
        }
        self.pend.push_back((frame_idx, slot));
        Ok(done)
    }

    /// 等待并返回所有在途帧（应在所有 worker 结束后调用，主线程锁外发送）。
    pub fn flush(&mut self) -> Vec<(usize, Vec<u8>)> {
        let mut out = Vec::new();
        while let Some((f, s)) = self.pend.pop_front() {
            if let Some(d) = self.emit(f, s) {
                out.push(d);
            }
        }
        out
    }

    /// 释放 channel 发送端（主线程收尾时调用，令接收循环得以结束）。
    pub fn close_tx(&mut self) {
        self.tx = None;
    }

    /// 在途帧数（供收尾判断）。
    pub fn pending(&self) -> usize {
        self.pend.len()
    }
}

/// GPU 是否可用（默认构建 false；feature 下 DLL 加载成功才 true）。
#[cfg(feature = "gpu")]
pub fn gpu_available() -> bool {
    gpu().is_some()
}
#[cfg(not(feature = "gpu"))]
pub fn gpu_available() -> bool {
    false
}

/// 是否显式启用 GPU 路径（`HDRCONV_GPU=1`；避免破坏默认 CPU 对齐）。
#[cfg(feature = "gpu")]
pub fn gpu_enabled() -> bool {
    std::env::var("HDRCONV_GPU").map(|v| v == "1").unwrap_or(false)
}
#[cfg(not(feature = "gpu"))]
pub fn gpu_enabled() -> bool {
    false
}

#[cfg(feature = "gpu")]
fn gpu() -> Option<&'static bindings::Gpu> {
    static GPU: OnceLock<Option<bindings::Gpu>> = OnceLock::new();
    GPU.get_or_init(|| {
        for candidate in [
            "backend/cuda/hdr_gpu_ffi.dll",
            "../backend/cuda/hdr_gpu_ffi.dll",
            "../../backend/cuda/hdr_gpu_ffi.dll",
            "../../../backend/cuda/hdr_gpu_ffi.dll",
        ] {
            if let Some(g) = bindings::Gpu::try_load(candidate) {
                return Some(g);
            }
        }
        None
    })
    .as_ref()
}

// ============================================================
//  管线接入点（默认 CPU；HDRCONV_GPU=1 + feature 时走 GPU，失败回退 CPU）
//  非 feature 构建为编译期 stub（返回 None），调用方无需条件编译。
// ============================================================

/// PNG / jpg_icc 像素链路：Rec.2020/PQ 变换（GPU 版）。
/// 返回 Some(像素) 表示 GPU 成功；None → 调用方回退 CPU。
#[cfg(feature = "gpu")]
pub fn try_gpu_rec2020_pq(img: &ImageData, settings: &Settings) -> Option<Vec<u8>> {
    if !gpu_enabled() {
        return None;
    }
    let g = gpu()?;
    let mut out = vec![0u8; img.pixels.len()];
    let exposure = settings.peak_nits / settings.white_nits;
    let ok = g.apply_hdr_rec2020_pq(
        &img.pixels,
        img.width,
        img.height,
        exposure,
        settings.gamma,
        settings.rgb.red,
        settings.rgb.green,
        settings.rgb.blue,
        settings.white_nits,
        &mut out,
    );
    if ok {
        Some(out)
    } else {
        eprintln!("[gpu] rec2020pq 失败：{}，回退 CPU", g.error_message());
        None
    }
}
#[cfg(not(feature = "gpu"))]
pub fn try_gpu_rec2020_pq(_img: &ImageData, _settings: &Settings) -> Option<Vec<u8>> {
    None
}

/// Ultra HDR：主图 sRGB→Display-P3（GPU 版）。
#[cfg(feature = "gpu")]
pub fn try_gpu_srgb_to_p3(rgba: &[u8], w: u32, h: u32) -> Option<Vec<u8>> {
    if !gpu_enabled() {
        return None;
    }
    let g = gpu()?;
    let mut out = vec![0u8; rgba.len()];
    if g.srgb_to_p3(rgba, w, h, &mut out) {
        Some(out)
    } else {
        eprintln!("[gpu] srgb_to_p3 失败：{}，回退 CPU", g.error_message());
        None
    }
}
#[cfg(not(feature = "gpu"))]
pub fn try_gpu_srgb_to_p3(_rgba: &[u8], _w: u32, _h: u32) -> Option<Vec<u8>> {
    None
}

/// Ultra HDR：增益图（GPU 版）。返回 (gm8, minContentBoost, maxContentBoost)。
#[cfg(feature = "gpu")]
pub fn try_gpu_compute_gainmap(
    rgba: &[u8],
    w: u32,
    h: u32,
    hdr_intensity_ev: f64,
    gamma: f64,
) -> Option<(Vec<u8>, f64, f64)> {
    if !gpu_enabled() {
        return None;
    }
    let g = gpu()?;
    let mut gm8 = vec![0u8; w as usize * h as usize];
    let mut minmax = [0.0f64; 2];
    if g.compute_gainmap(rgba, w, h, hdr_intensity_ev, gamma, &mut gm8, &mut minmax) {
        Some((gm8, minmax[0], minmax[1]))
    } else {
        eprintln!("[gpu] compute_gainmap 失败：{}，回退 CPU", g.error_message());
        None
    }
}
#[cfg(not(feature = "gpu"))]
pub fn try_gpu_compute_gainmap(
    _rgba: &[u8],
    _w: u32,
    _h: u32,
    _hdr_intensity_ev: f64,
    _gamma: f64,
) -> Option<(Vec<u8>, f64, f64)> {
    None
}

/// 视频逐帧：增益图重建 → n*6 大端 16-bit 像素（GPU 版；PAM 头由调用方补）。
#[cfg(feature = "gpu")]
pub fn try_gpu_reconstruct_gainmap16_pixels(
    rgba: &[u8],
    w: u32,
    h: u32,
    hdr_intensity_ev: f64,
    gamma: f64,
    peak: f64,
) -> Option<Vec<u8>> {
    if !gpu_enabled() {
        return None;
    }
    let g = gpu()?;
    let mut out = vec![0u8; w as usize * h as usize * 6];
    if g.reconstruct_gainmap16(rgba, w, h, hdr_intensity_ev, gamma, peak, &mut out) {
        Some(out)
    } else {
        eprintln!("[gpu] reconstruct_gainmap16 失败：{}，回退 CPU", g.error_message());
        None
    }
}
#[cfg(not(feature = "gpu"))]
pub fn try_gpu_reconstruct_gainmap16_pixels(
    _rgba: &[u8],
    _w: u32,
    _h: u32,
    _hdr_intensity_ev: f64,
    _gamma: f64,
    _peak: f64,
) -> Option<Vec<u8>> {
    None
}

/// 视频逐帧：单层变换重建 → n*6 大端 16-bit 像素（GPU 版）。
#[cfg(feature = "gpu")]
pub fn try_gpu_reconstruct_transform16_pixels(
    rgba: &[u8],
    w: u32,
    h: u32,
    exposure: f64,
    gamma: f64,
    r_adj: f64,
    g_adj: f64,
    b_adj: f64,
    peak: f64,
) -> Option<Vec<u8>> {
    if !gpu_enabled() {
        return None;
    }
    let g = gpu()?;
    let mut out = vec![0u8; w as usize * h as usize * 6];
    if g.reconstruct_transform16(rgba, w, h, exposure, gamma, r_adj, g_adj, b_adj, peak, &mut out) {
        Some(out)
    } else {
        eprintln!("[gpu] reconstruct_transform16 失败：{}，回退 CPU", g.error_message());
        None
    }
}
#[cfg(not(feature = "gpu"))]
pub fn try_gpu_reconstruct_transform16_pixels(
    _rgba: &[u8],
    _w: u32,
    _h: u32,
    _exposure: f64,
    _gamma: f64,
    _r_adj: f64,
    _g_adj: f64,
    _b_adj: f64,
    _peak: f64,
) -> Option<Vec<u8>> {
    None
}

// ============================================================
//  非 feature 构建的 FramePump / FrameMode stub（调用方无需条件编译）
// ============================================================

#[cfg(not(feature = "gpu"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameMode {
    Gainmap16,
    Transform16,
}

#[cfg(not(feature = "gpu"))]
pub struct FramePump;

#[cfg(not(feature = "gpu"))]
impl FramePump {
    pub fn try_new(_tx: std::sync::mpsc::SyncSender<(usize, Vec<u8>)>) -> Option<Self> {
        None
    }
    pub fn submit(
        &mut self,
        _frame_idx: usize,
        _rgba: &[u8],
        _w: u32,
        _h: u32,
        _mode: FrameMode,
        _params: &[f64],
    ) -> anyhow::Result<Vec<(usize, Vec<u8>)>> {
        Err(anyhow::anyhow!("GPU 帧管线不可用（未启用 feature gpu）"))
    }
    pub fn flush(&mut self) -> Vec<(usize, Vec<u8>)> {
        Vec::new()
    }
    pub fn close_tx(&mut self) {}
    pub fn pending(&self) -> usize {
        0
    }
}