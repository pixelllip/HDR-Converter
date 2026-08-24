//! ← HdrGpuJni.kt + backend/cuda/include/hdr_gpu.h：GPU 加速（可选，feature "gpu"）。
//!
//! ## 现状（2026-08 实证）
//! `backend/cuda/hdr_gpu_jni.dll` **只导出 JNI 符号**（`Java_com_hdrconverter_HdrGpuJni_*`，
//! 用 `examples/dump_exports.rs` + object crate 枚举确认共 9 个），**不含 `hdr_gpu_*` C ABI 导出**
//! ——尽管 `hdr_gpu.h` 已声明该 C ABI。因此：
//! - 本模块按 `hdr_gpu.h` 的 C ABI 绑定 FFI（libloading 运行时加载）；
//! - 当前 DLL 加载后 `get(b"hdr_gpu_init")` 会失败 → `gpu_available()=false` → CPU 回退（默认行为）；
//! - **启用前提（CUDA 侧任务）**：让 backend/cuda 编译一个额外导出 C ABI 的 DLL
//!   （编译实现源时定义 `HDR_GPU_EXPORTS` 使 `hdr_gpu_*` 成为 `__declspec(dllexport)`，
//!   或增加一个纯 C 导出包装），产物打包时与 ffmpeg 一样放 asarUnpack。
//!
//! ## 覆盖范围的诚实说明
//! `hdr_gpu_process`（C ABI 唯一处理函数）语义对应当前管线的 **legacy applyHdrTransform**
//! （sRGB 输出的自动伽马版，/convert 已不再使用）。要让 GPU 加速 **Rec.2020/PQ 图片链路**
//! 或 **增益图 / 逐帧 16-bit 重建**，需在 CUDA 侧按 `HdrGpuJni.kt` 的 native 方法
//! （nativeApplyHdrTransformToRec2020Pq / nativeComputeGainMap / nativeReconstructFrameGainMap16 /
//! nativeReconstructFrameTransform16 / nativeSrgbToDisplayP3）扩展 C ABI——这是独立的 CUDA 开发任务。

/// C ABI 契约（逐一对齐 backend/cuda/include/hdr_gpu.h，全部 `extern "C"`）：
///
/// ```c
/// HDR_GPU_API int  hdr_gpu_init(int backend);       // 0=CUDA 优先自动检测, 1=CUDA, 2=DirectCompute
/// HDR_GPU_API int  hdr_gpu_process(const unsigned char* input, int w, int h,
///                                  unsigned char* output, float totalExposure, float gamma,
///                                  float rAdj, float gAdj, float bAdj);
/// HDR_GPU_API const char* hdr_gpu_error();
/// HDR_GPU_API void hdr_gpu_cleanup();
/// HDR_GPU_API int  hdr_gpu_backend();
/// ```

#[cfg(feature = "gpu")]
pub mod bindings {
    use libloading::Library;
    use std::ffi::{c_char, c_int, CStr};

    type InitFn = unsafe extern "C" fn(c_int) -> c_int;
    type ProcessFn = unsafe extern "C" fn(
        *const u8,
        c_int,
        c_int,
        *mut u8,
        f32,
        f32,
        f32,
        f32,
        f32,
    ) -> c_int;
    type ErrorFn = unsafe extern "C" fn() -> *const c_char;
    type CleanupFn = unsafe extern "C" fn();
    type BackendFn = unsafe extern "C" fn() -> c_int;

    /// GPU 加速器句柄（进程内单例使用）。
    /// 存裸函数指针（解引用自 Symbol）而非 Symbol 本身，避免 Library 自引用生命周期；
    /// `_lib` 保持 DLL 存活，指针在 drop 前始终有效。
    pub struct Gpu {
        _lib: Library,
        #[allow(dead_code)]
        backend: c_int,
        process: ProcessFn,
        error: ErrorFn,
        cleanup: CleanupFn,
        backend_fn: BackendFn,
    }

    // 函数指针只读，跨线程安全
    unsafe impl Send for Gpu {}
    unsafe impl Sync for Gpu {}

    impl Gpu {
        /// 加载 DLL 并初始化（优先 CUDA，失败回退 DirectCompute，再失败返回 None → CPU）。
        pub fn try_load(dll_path: &str) -> Option<Self> {
            let lib = unsafe { Library::new(dll_path) }.ok()?;
            let init: InitFn = unsafe { *lib.get::<InitFn>(b"hdr_gpu_init").ok()? };
            let mut backend = 0;
            unsafe {
                for candidate in [1, 2] {
                    if init(candidate) == 0 {
                        backend = candidate;
                        break;
                    }
                }
            }
            if backend == 0 {
                return None;
            }
            let process: ProcessFn = unsafe { *lib.get::<ProcessFn>(b"hdr_gpu_process").ok()? };
            let error: ErrorFn = unsafe { *lib.get::<ErrorFn>(b"hdr_gpu_error").ok()? };
            let cleanup: CleanupFn = unsafe { *lib.get::<CleanupFn>(b"hdr_gpu_cleanup").ok()? };
            let backend_fn: BackendFn = unsafe { *lib.get::<BackendFn>(b"hdr_gpu_backend").ok()? };
            Some(Gpu {
                _lib: lib,
                backend,
                process,
                error,
                cleanup,
                backend_fn,
            })
        }

        /// 处理图像（RGBA 8-bit in/out；语义 = legacy applyHdrTransform，sRGB 输出）。
        /// 返回 false 时调用侧应回退 CPU。
        pub fn process(
            &self,
            input: &[u8],
            width: c_int,
            height: c_int,
            output: &mut [u8],
            total_exposure: f32,
            gamma: f32,
            r_adj: f32,
            g_adj: f32,
            b_adj: f32,
        ) -> bool {
            unsafe {
                (self.process)(
                    input.as_ptr(),
                    width,
                    height,
                    output.as_mut_ptr(),
                    total_exposure,
                    gamma,
                    r_adj,
                    g_adj,
                    b_adj,
                ) == 0
            }
        }

        /// 上次错误消息。
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

        /// 当前后端类型（1=CUDA, 2=DirectCompute）。
        pub fn backend(&self) -> c_int {
            unsafe { (self.backend_fn)() }
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

/// GPU 是否可用（默认编译不含 GPU → false；含 feature 但 DLL 无 C ABI 导出 → false）。
pub fn gpu_available() -> bool {
    #[cfg(feature = "gpu")]
    {
        static AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *AVAILABLE.get_or_init(|| {
            // 开发路径候选；打包后需按 asarUnpack 解析（同 ffmpeg 处理）
            for candidate in [
                "backend/cuda/hdr_gpu_jni.dll",
                "../backend/cuda/hdr_gpu_jni.dll",
                "../../backend/cuda/hdr_gpu_jni.dll",
            ] {
                if bindings::Gpu::try_load(candidate).is_some() {
                    return true;
                }
            }
            false
        })
    }
    #[cfg(not(feature = "gpu"))]
    {
        false
    }
}