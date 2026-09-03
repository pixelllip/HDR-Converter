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
    /// masked 变体（视频链路 2 修复）：输入 mask 表（f64，全分辨率，软阈值），
    /// GPU 仅做 RGB*gain（不再算硬阈值 mask）。避免帧间 flicker。
    type Reconstruct16MaskedFn = unsafe extern "C" fn(
        *const u8,
        *const f64,
        c_int,
        c_int,
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
        reconstruct_gainmap16_masked: Reconstruct16MaskedFn,
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
            let reconstruct_gainmap16_masked = get(&lib, b"hdr_ffi_reconstruct_gainmap16_masked")?;
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
                reconstruct_gainmap16_masked,
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

        /// 视频链路 2 修复版：host 预算全分辨率软阈值 mask，GPU 仅做 RGB*gain。
        pub fn reconstruct_gainmap16_masked(
            &self,
            rgba: &[u8],
            mask_full: &[f64],
            w: u32,
            h: u32,
            hdr_intensity: f64,
            peak: f64,
            out: &mut [u8],
        ) -> bool {
            if out.len() != w as usize * h as usize * 6
                || mask_full.len() != w as usize * h as usize
            {
                return false;
            }
            unsafe {
                (self.reconstruct_gainmap16_masked)(
                    rgba.as_ptr(),
                    mask_full.as_ptr(),
                    w as c_int,
                    h as c_int,
                    hdr_intensity,
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

/// 异步帧管线模式（对齐 hdr_gpu_ffi.cu：0=gainmap16, 1=transform16, 2=gainmap16_masked）。
#[cfg(feature = "gpu")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameMode {
    Gainmap16,
    Transform16,
    /// 视频链路 2 修复版：host 预算软阈值 mask 表传给 GPU，消除帧间 flicker。
    /// `params` 应为 `[hdrIntensity, peak, <f64 指针拆分为两个 u32 低位/高位>]`，
    /// `submit_masked` 会把 `mask` 指针安全拼到 params[2..=3]。
    Gainmap16Masked,
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
    slot_dims: [(u32, u32); MAX_FRAME_SLOTS],
    next_slot: usize,
    slots: usize,
}

#[cfg(feature = "gpu")]
const MAX_FRAME_SLOTS: usize = 8;

#[cfg(feature = "gpu")]
impl FramePump {
    /// 槽数 = `HDRCONV_GPU_SLOTS`（1..=MAX_FRAME_SLOTS，默认 2）。
    /// 实测曲线（60 帧 4K，2026-08）：1 槽 7.6s/726MB、2 槽 7.5s/1225MB、
    /// 4 槽 7.2s/1394MB、8 槽 7.3s/1700MB → 2~4 槽封顶（SM/PCIe 饱和），
    /// 默认 2 兼顾速度与内存（双缓冲），可显式调大。
    pub fn try_new(tx: std::sync::mpsc::SyncSender<(usize, Vec<u8>)>) -> Option<Self> {
        let g = gpu()?;
        let slots = std::env::var("HDRCONV_GPU_SLOTS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(2)
            .clamp(1, MAX_FRAME_SLOTS);
        Some(FramePump {
            g,
            tx: Some(tx),
            pend: std::collections::VecDeque::new(),
            slot_dims: [(0, 0); MAX_FRAME_SLOTS],
            next_slot: 0,
            slots,
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
    ///
    /// 当 mode = `Gainmap16Masked` 时，需要传入 `mask_full`（f64，全分辨率，软阈值 mask）。
    /// `params` 此时为 `[hdrIntensity, peak]`（仅 2 个元素，mask 指针由本函数内部拼）。
    pub fn submit(
        &mut self,
        frame_idx: usize,
        rgba: &[u8],
        w: u32,
        h: u32,
        mode: FrameMode,
        params: &[f64],
        mask_full: Option<&[f64]>,
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

        // Gainmap16Masked 模式需要把 mask 指针拼到 params[2..=3]。
        let mut owned_params: Vec<f64> = Vec::new();
        let params_slice: &[f64] = match mode {
            FrameMode::Gainmap16Masked => {
                let mask = mask_full.ok_or_else(|| {
                    anyhow::anyhow!("[gpu] frame_submit mode=Gainmap16Masked 必须传 mask_full")
                })?;
                if mask.len() != (w * h) as usize {
                    return Err(anyhow::anyhow!(
                        "[gpu] frame_submit mask 长度不匹配：mask={}, w*h={}",
                        mask.len(),
                        (w * h) as usize
                    ));
                }
                let mask_ptr = mask.as_ptr() as usize;
                let mp = mask_ptr as u64;
                let lo = (mp & 0xFFFFFFFFu64) as u32;
                let hi = ((mp >> 32) & 0xFFFFFFFFu64) as u32;
                owned_params.clear();
                owned_params.push(params[0]); // hdrIntensity
                owned_params.push(params[1]); // peak
                let lo_f = f64::from_bits(lo as u64);
                let hi_f = f64::from_bits(hi as u64);
                owned_params.push(lo_f);
                owned_params.push(hi_f);
                owned_params.as_slice()
            }
            _ => params,
        };
        let mode_num = match mode {
            FrameMode::Gainmap16 => 0,
            FrameMode::Transform16 => 1,
            FrameMode::Gainmap16Masked => 2,
        };
        if !self
            .g
            .frame_submit(slot as c_int, rgba, mode_num, params_slice)
        {
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

/// Ultra HDR：增益图（GPU 版）。返回 **低分辨率** (gm_w, gm_h) 8-bit 增益图。
///
/// 第二步实现（low-res gain map）：
/// 先在 Rust 端 box-average 下采样主图 RGBA 到 (w/4, h/4)，再把低分辨率 RGBA + 尺寸
/// 传给 `hdr_ffi_compute_gainmap`，让 CUDA 在低分辨率上算 mask/gain/8-bit 量化。
/// 输出 buffer 字节数 = gm_w * gm_h。CUDA 内核不需要改。
///
/// 返回 (gm8, minContentBoost, maxContentBoost)。
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
    let gm_w = (w / 4).max(1);
    let gm_h = (h / 4).max(1);
    // 第 0 步：box-average 下采样主图到低分辨率（与 CPU 链路一致）
    let low_rgba =
        crate::ultra_hdr::downscale_area_average_box_rgba(rgba, w as usize, h as usize, gm_w as usize, gm_h as usize);
    let mut gm8 = vec![0u8; gm_w as usize * gm_h as usize];
    let mut minmax = [0.0f64; 2];
    if g.compute_gainmap(&low_rgba, gm_w, gm_h, hdr_intensity_ev, gamma, &mut gm8, &mut minmax) {
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
///
/// **修复（与图片 Ultra HDR 三步修复对齐）**：host 端先 box 下采样主图到 1/4 → 低分辨率
/// 硬阈值 mask → 3×3 高斯模糊 → 双线性 4× 上采样回原分辨率 → 把 mask 表传给 GPU，
/// GPU 仅做 RGB*gain。这样 mask 在原分辨率上是平滑、低频的，过渡带宽 ≈ 8~12 主图像素，
/// 帧间亮度抖动时像素不会在 gain=1 ↔ gain>1 之间反复切换——消除视频链路 flicker。
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
    // host 端预算全分辨率软阈值 mask（与 CPU 链路 `reconstruct_linear_hdr_frame` 共用管线）
    let (mask_low, gm_w, gm_h) =
        crate::ultra_hdr::compute_lowres_soft_mask(rgba, w as usize, h as usize, gamma);
    let mask_full = crate::ultra_hdr::upscale_bilinear_f64(mask_low.as_slice(), gm_w, gm_h, w as usize, h as usize);
    if g.reconstruct_gainmap16_masked(rgba, &mask_full, w, h, hdr_intensity_ev, peak, &mut out) {
        Some(out)
    } else {
        eprintln!(
            "[gpu] reconstruct_gainmap16_masked 失败：{}，回退 CPU",
            g.error_message()
        );
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
    /// 与 feature "gpu" 下的 Gainmap16Masked 对齐；非 GPU 构建时调用方不应使用。
    Gainmap16Masked,
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
        _mask_full: Option<&[f64]>,
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