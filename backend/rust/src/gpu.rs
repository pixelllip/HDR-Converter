//! ← HdrGpuJni.kt + backend/cuda/include/hdr_gpu.h：GPU 加速（可选）。
//!
//! C ABI 契约见 `backend/cuda/include/hdr_gpu.h`：
//! ```c
//! int  hdr_gpu_init(int backend);                    // 0=CUDA 优先自动检测, 1=CUDA, 2=DirectCompute
//! int  hdr_gpu_process(const unsigned char* input, int w, int h,
//!                      unsigned char* output, float totalExposure, float gamma,
//!                      float rAdj, float gAdj, float bAdj);
//! const char* hdr_gpu_error();
//! void hdr_gpu_cleanup();
//! ```
//!
//! 注意：`backend/cuda/hdr_gpu_jni.dll` 同时含 JNI 接口（hdr_gpu_jni.cu 包装）与
//! C ABI（hdr_gpu.h）；启用本模块前需用 dumpbin /exports 确认导出符号名。
//! DLL 运行时路径：开发态为 `backend/cuda/`，打包后位于 asarUnpack（同主进程 JAR 处理）。

#[cfg(feature = "gpu")]
pub mod bindings {
    use libloading::{Library, Symbol};
    use std::ffi::c_int;

    /// GPU 加速器句柄（进程内单例使用）。
    pub struct Gpu {
        _lib: Library,
        backend: c_int,
    }

    impl Gpu {
        /// 加载 DLL 并初始化（优先 CUDA，失败回退 DirectCompute，再失败返回 None → CPU）。
        pub fn try_load(dll_path: &str) -> Option<Self> {
            let lib = Library::new(dll_path).ok()?;
            unsafe {
                let init: Symbol<unsafe extern "C" fn(c_int) -> c_int> =
                    lib.get(b"hdr_gpu_init").ok()?;
                let mut backend = 0;
                for candidate in [1, 2] {
                    if init(candidate) == 0 {
                        backend = candidate;
                        break;
                    }
                }
                if backend == 0 {
                    return None;
                }
                Some(Gpu { _lib: lib, backend })
            }
        }

        /// 处理图像（RGBA 8-bit in/out）。返回 false 时调用侧应回退 CPU。
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
                let proc: Symbol<
                    unsafe extern "C" fn(
                        *const u8,
                        c_int,
                        c_int,
                        *mut u8,
                        f32,
                        f32,
                        f32,
                        f32,
                        f32,
                    ) -> c_int,
                > = match self._lib.get(b"hdr_gpu_process") {
                    Ok(s) => s,
                    Err(_) => return false,
                };
                proc(
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

        pub fn backend(&self) -> c_int {
            self.backend
        }
    }
}

/// GPU 是否可用（默认编译不含 GPU → false；CPU 回退路径始终可用）。
#[cfg(feature = "gpu")]
pub fn gpu_available() -> bool {
    bindings::Gpu::try_load("backend/cuda/hdr_gpu_jni.dll").is_some()
}

/// GPU 是否可用（未启用 feature "gpu"）。
#[cfg(not(feature = "gpu"))]
pub fn gpu_available() -> bool {
    false
}