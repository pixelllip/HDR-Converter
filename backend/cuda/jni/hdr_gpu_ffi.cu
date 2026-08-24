// ============================================================================
// hdr_gpu_ffi.cu —— C ABI 导出（供 Rust hdrconv --features gpu 通过 libloading FFI）
//
// 复用 hdr_gpu_jni.cu 的全部内核与 host 逻辑（同一编译单元 include），
// 额外导出纯 C ABI 函数；语义与 JNI 包装逐字一致（kernel/分配/归约完全相同）。
//
// 构建（见 build_ffi.bat，与 build_jni.bat 同参数）：
//   nvcc -shared -O3 -gencode arch=compute_75,code=compute_75 -cudart=static \
//        -I"%JAVA_HOME%\include" -I"%JAVA_HOME%\include\win32" \
//        -Xcompiler "/MD /wd4819" hdr_gpu_ffi.cu -o hdr_gpu_ffi.dll
//
// 产物导出：hdr_ffi_*（C ABI）+ Java_*（JNI，保留兼容）
// ============================================================================

#include "hdr_gpu_jni.cu"

#include <mutex>
#include <string>

static std::mutex g_ffiMutex;
static std::string g_ffiError;

static void setFfiError(const char *m)
{
    std::lock_guard<std::mutex> L(g_ffiMutex);
    g_ffiError = m ? m : "";
    if (m)
        fprintf(stderr, "[hdr_gpu_ffi] error: %s\n", m);
}

#define FFI_API __declspec(dllexport)

// 异步帧管线槽位数（固定 4，字面量使用以兼容 nvcc 两遍编译的符号可见性）
#define ASYNC_SLOTS 8
#define ASYNC_SLOTS_STR "8"

extern "C"
{
    FFI_API void hdr_ffi_frame_release(int slot); // 前向声明（C 链接，与定义一致）

    // ---- 生命周期 ----
    FFI_API int hdr_ffi_init(int /*backend*/)
    {
        cudaError_t e = cudaSetDevice(0);
        if (e != cudaSuccess)
        {
            setFfiError(cudaGetErrorString(e));
            return -1;
        }
        cudaFree(nullptr); // 触发上下文初始化
        cudaDeviceProp prop;
        if (cudaGetDeviceProperties(&prop, 0) == cudaSuccess)
        {
            fprintf(stderr, "[hdr_gpu_ffi] CUDA ready: %s (sm_%d%d, %.1f GB)\n",
                    prop.name, prop.major, prop.minor, prop.totalGlobalMem / (1024.0 * 1024.0 * 1024.0));
        }
        return 0;
    }

    FFI_API void hdr_ffi_cleanup()
    {
        for (int i = 0; i < ASYNC_SLOTS; i++)
            hdr_ffi_frame_release(i);
        cudaDeviceReset();
    }

    FFI_API const char *hdr_ffi_error()
    {
        std::lock_guard<std::mutex> L(g_ffiMutex);
        return g_ffiError.c_str();
    }

    FFI_API int hdr_ffi_backend()
    {
        return 1; // CUDA
    }

    // ---- sRGB -> Display-P3（主图像素，sRGB 传递）----
    FFI_API int hdr_ffi_srgb_to_p3(const unsigned char *rgba, int w, int h, unsigned char *out)
    {
        int n = w * h;
        int blocks = numBlocks(n);
        uchar4 *dIn = nullptr, *dOut = nullptr;
        cudaError_t e = cudaMalloc(&dIn, (size_t)n * 4);
        if (e == cudaSuccess)
            e = cudaMalloc(&dOut, (size_t)n * 4);
        if (e == cudaSuccess)
            e = cudaMemcpy(dIn, rgba, (size_t)n * 4, cudaMemcpyHostToDevice);
        if (e == cudaSuccess)
        {
            kSrgbToP3<<<blocks, THREADS>>>(dIn, dOut, n);
            e = cudaGetLastError();
            if (e == cudaSuccess)
                e = cudaDeviceSynchronize();
        }
        if (e == cudaSuccess)
            e = cudaMemcpy(out, dOut, (size_t)n * 4, cudaMemcpyDeviceToHost);
        if (dOut)
            cudaFree(dOut);
        if (dIn)
            cudaFree(dIn);
        if (e != cudaSuccess)
        {
            setFfiError(cudaGetErrorString(e));
            return -3;
        }
        return 0;
    }

    // ---- 增益图（outMinMax[0]=minBoost, [1]=maxBoostActual）----
    FFI_API int hdr_ffi_compute_gainmap(const unsigned char *rgba, int w, int h,
                                        double hdrIntensity, double gamma,
                                        unsigned char *gm8, double *outMinMax)
    {
        int n = w * h;
        int blocks = numBlocks(n);
        double maxBoostD = pow(2.0, hdrIntensity);
        if (maxBoostD < 1.0)
            maxBoostD = 1.0;
        if (maxBoostD > 64.0)
            maxBoostD = 64.0;
        float maxBoost = (float)maxBoostD;
        float gammaF = (float)gamma;
        const float highlightStart = 0.5f;
        const float offset = 1.0f / 64.0f;

        uchar4 *dIn = nullptr;
        float *dGain = nullptr;
        unsigned char *dGm8 = nullptr;
        float *dMin = nullptr, *dMax = nullptr;
        cudaError_t e = cudaMalloc(&dIn, (size_t)n * 4);
        if (e == cudaSuccess)
            e = cudaMalloc(&dGain, (size_t)n * sizeof(float));
        if (e == cudaSuccess)
            e = cudaMalloc(&dGm8, (size_t)n);
        if (e == cudaSuccess)
            e = cudaMalloc(&dMin, (size_t)blocks * sizeof(float));
        if (e == cudaSuccess)
            e = cudaMalloc(&dMax, (size_t)blocks * sizeof(float));
        if (e == cudaSuccess)
            e = cudaMemcpy(dIn, rgba, (size_t)n * 4, cudaMemcpyHostToDevice);
        if (e == cudaSuccess)
        {
            size_t shmem = THREADS * sizeof(float);
            kGainMapPhase1<<<blocks, THREADS, shmem>>>(dIn, dGain, dMin, dMax, n, maxBoost, gammaF, highlightStart, offset);
            e = cudaGetLastError();
            if (e == cudaSuccess)
                e = cudaDeviceSynchronize();
        }

        float gmin = 1e30f, gmax = -1e30f;
        if (e == cudaSuccess)
        {
            std::vector<float> hMin(blocks), hMax(blocks);
            e = cudaMemcpy(hMin.data(), dMin, (size_t)blocks * sizeof(float), cudaMemcpyDeviceToHost);
            if (e == cudaSuccess)
                e = cudaMemcpy(hMax.data(), dMax, (size_t)blocks * sizeof(float), cudaMemcpyDeviceToHost);
            for (int b = 0; b < blocks && e == cudaSuccess; b++)
            {
                if (hMin[b] < gmin)
                    gmin = hMin[b];
                if (hMax[b] > gmax)
                    gmax = hMax[b];
            }
        }

        double minBoost = 1.0, maxBoostActual = 1.0;
        if (e == cudaSuccess)
        {
            double dMinB = fmin(1.0, (double)gmin);
            if (dMinB < 0.25)
                dMinB = 0.25;
            if (dMinB > 1.0)
                dMinB = 1.0;
            minBoost = dMinB;
            maxBoostActual = fmax(1.0, (double)gmax);
            double mapMin = log(minBoost) / log(2.0);
            double mapMax = log(maxBoostActual) / log(2.0);
            double range = fmax(mapMax - mapMin, 1e-6);
            kGainMapPhase2<<<blocks, THREADS>>>(dGain, dGm8, n, (float)mapMin, (float)mapMax, (float)range);
            e = cudaGetLastError();
            if (e == cudaSuccess)
                e = cudaDeviceSynchronize();
        }
        if (e == cudaSuccess)
            e = cudaMemcpy(gm8, dGm8, (size_t)n, cudaMemcpyDeviceToHost);
        if (e == cudaSuccess)
        {
            outMinMax[0] = minBoost;
            outMinMax[1] = maxBoostActual;
        }

        if (dMax)
            cudaFree(dMax);
        if (dMin)
            cudaFree(dMin);
        if (dGm8)
            cudaFree(dGm8);
        if (dGain)
            cudaFree(dGain);
        if (dIn)
            cudaFree(dIn);
        if (e != cudaSuccess)
        {
            setFfiError(cudaGetErrorString(e));
            return -3;
        }
        return 0;
    }

    // ---- 图片 PNG 路径色调映射（自动伽马 + 曝光 + 通道调整；legacy applyHdrTransform）----
    FFI_API int hdr_ffi_apply_hdr_transform(const unsigned char *rgba, int w, int h,
                                            double totalExposure, double gamma,
                                            double rAdj, double gAdj, double bAdj,
                                            unsigned char *out)
    {
        int n = w * h;
        int blocks = numBlocks(n);
        uchar4 *dIn = nullptr;
        float3 *dLinear = nullptr;
        uchar4 *dOut = nullptr;
        float *dLum = nullptr;
        cudaError_t e = cudaMalloc(&dIn, (size_t)n * 4);
        if (e == cudaSuccess)
            e = cudaMalloc(&dLinear, (size_t)n * sizeof(float3));
        if (e == cudaSuccess)
            e = cudaMalloc(&dOut, (size_t)n * 4);
        if (e == cudaSuccess)
            e = cudaMalloc(&dLum, (size_t)blocks * sizeof(float));
        if (e == cudaSuccess)
            e = cudaMemcpy(dIn, rgba, (size_t)n * 4, cudaMemcpyHostToDevice);
        if (e == cudaSuccess)
        {
            size_t shmem = THREADS * sizeof(float);
            kHdrTransformPhase1<<<blocks, THREADS, shmem>>>(dIn, dLinear, dLum, n);
            e = cudaGetLastError();
            if (e == cudaSuccess)
                e = cudaDeviceSynchronize();
        }

        float autoGamma = 1.0f;
        if (e == cudaSuccess)
        {
            std::vector<float> hLum(blocks);
            e = cudaMemcpy(hLum.data(), dLum, (size_t)blocks * sizeof(float), cudaMemcpyDeviceToHost);
            double sum = 0.0;
            for (int b = 0; b < blocks; b++)
                sum += hLum[b];
            double mean = sum / n;
            if (mean > 0.001 && mean < 0.999)
            {
                double ag = log(0.5) / log(mean);
                if (ag < 0.3)
                    ag = 0.3;
                if (ag > 3.0)
                    ag = 3.0;
                autoGamma = (float)ag;
            }
        }

        if (e == cudaSuccess)
        {
            kHdrTransformPhase2<<<blocks, THREADS>>>(
                dLinear, dOut, n, autoGamma, (float)totalExposure,
                (float)rAdj, (float)gAdj, (float)bAdj, (float)gamma);
            e = cudaGetLastError();
            if (e == cudaSuccess)
                e = cudaDeviceSynchronize();
        }
        if (e == cudaSuccess)
            e = cudaMemcpy(out, dOut, (size_t)n * 4, cudaMemcpyDeviceToHost);

        if (dLum)
            cudaFree(dLum);
        if (dOut)
            cudaFree(dOut);
        if (dLinear)
            cudaFree(dLinear);
        if (dIn)
            cudaFree(dIn);
        if (e != cudaSuccess)
        {
            setFfiError(cudaGetErrorString(e));
            return -3;
        }
        return 0;
    }

    // ---- Rec.2020/PQ 图片链路（png/jpg_icc；无自动伽马，与 CPU apply_hdr_rec2020_pq 对应）----
    FFI_API int hdr_ffi_apply_hdr_rec2020_pq(const unsigned char *rgba, int w, int h,
                                             double exposure, double gamma,
                                             double rAdj, double gAdj, double bAdj,
                                             double whiteNits, unsigned char *out)
    {
        int n = w * h;
        int blocks = numBlocks(n);
        uchar4 *dIn = nullptr;
        float3 *dLinear = nullptr;
        uchar4 *dOut = nullptr;
        float *dLum = nullptr;
        cudaError_t e = cudaMalloc(&dIn, (size_t)n * 4);
        if (e == cudaSuccess)
            e = cudaMalloc(&dLinear, (size_t)n * sizeof(float3));
        if (e == cudaSuccess)
            e = cudaMalloc(&dOut, (size_t)n * 4);
        if (e == cudaSuccess)
            e = cudaMalloc(&dLum, (size_t)blocks * sizeof(float));
        if (e == cudaSuccess)
            e = cudaMemcpy(dIn, rgba, (size_t)n * 4, cudaMemcpyHostToDevice);
        if (e == cudaSuccess)
        {
            size_t shmem = THREADS * sizeof(float);
            kHdrTransformPhase1<<<blocks, THREADS, shmem>>>(dIn, dLinear, dLum, n);
            e = cudaGetLastError();
            if (e == cudaSuccess)
                e = cudaDeviceSynchronize();
        }

        float autoGamma = 1.0f; // 无自动伽马（与视频预览一致）

        if (e == cudaSuccess)
        {
            float pqScale = (float)(whiteNits / 10000.0);
            kHdrTransformPhase2Rec2020Pq<<<blocks, THREADS>>>(
                dLinear, dOut, n, autoGamma, (float)exposure,
                (float)rAdj, (float)gAdj, (float)bAdj, (float)gamma, pqScale);
            e = cudaGetLastError();
            if (e == cudaSuccess)
                e = cudaDeviceSynchronize();
        }
        if (e == cudaSuccess)
            e = cudaMemcpy(out, dOut, (size_t)n * 4, cudaMemcpyDeviceToHost);

        if (dLum)
            cudaFree(dLum);
        if (dOut)
            cudaFree(dOut);
        if (dLinear)
            cudaFree(dLinear);
        if (dIn)
            cudaFree(dIn);
        if (e != cudaSuccess)
        {
            setFfiError(cudaGetErrorString(e));
            return -3;
        }
        return 0;
    }

    // ---- 逐帧增益图重建：输出线性 16-bit 大端（视频链路 2/gainmap）----
    FFI_API int hdr_ffi_reconstruct_gainmap16(const unsigned char *rgba, int w, int h,
                                              double hdrIntensity, double gamma, double peak,
                                              unsigned char *out)
    {
        int n = w * h;
        int blocks = numBlocks(n);
        double maxBoost = pow(2.0, hdrIntensity);
        if (maxBoost < 1.0)
            maxBoost = 1.0;
        if (maxBoost > 64.0)
            maxBoost = 64.0;
        uchar4 *dIn = nullptr;
        unsigned char *dOut = nullptr;
        cudaError_t e = cudaMalloc(&dIn, (size_t)n * 4);
        if (e == cudaSuccess)
            e = cudaMalloc(&dOut, (size_t)n * 6);
        if (e == cudaSuccess)
            e = cudaMemcpy(dIn, rgba, (size_t)n * 4, cudaMemcpyHostToDevice);
        if (e == cudaSuccess)
        {
            kFrameGainMap16<<<blocks, THREADS>>>(dIn, dOut, n, maxBoost, gamma, peak);
            e = cudaGetLastError();
            if (e == cudaSuccess)
                e = cudaDeviceSynchronize();
        }
        if (e == cudaSuccess)
            e = cudaMemcpy(out, dOut, (size_t)n * 6, cudaMemcpyDeviceToHost);
        if (dOut)
            cudaFree(dOut);
        if (dIn)
            cudaFree(dIn);
        if (e != cudaSuccess)
        {
            setFfiError(cudaGetErrorString(e));
            return -3;
        }
        return 0;
    }

    // ---- 逐帧单层变换：输出线性 16-bit 大端（视频链路 1/direct）----
    FFI_API int hdr_ffi_reconstruct_transform16(const unsigned char *rgba, int w, int h,
                                                double exposure, double gamma,
                                                double rAdj, double gAdj, double bAdj,
                                                double peak, unsigned char *out)
    {
        int n = w * h;
        int blocks = numBlocks(n);
        uchar4 *dIn = nullptr;
        unsigned char *dOut = nullptr;
        cudaError_t e = cudaMalloc(&dIn, (size_t)n * 4);
        if (e == cudaSuccess)
            e = cudaMalloc(&dOut, (size_t)n * 6);
        if (e == cudaSuccess)
            e = cudaMemcpy(dIn, rgba, (size_t)n * 4, cudaMemcpyHostToDevice);
        if (e == cudaSuccess)
        {
            kFrameTransform16<<<blocks, THREADS>>>(dIn, dOut, n, exposure, gamma, rAdj, gAdj, bAdj, peak);
            e = cudaGetLastError();
            if (e == cudaSuccess)
                e = cudaDeviceSynchronize();
        }
        if (e == cudaSuccess)
            e = cudaMemcpy(out, dOut, (size_t)n * 6, cudaMemcpyDeviceToHost);
        if (dOut)
            cudaFree(dOut);
        if (dIn)
            cudaFree(dIn);
        if (e != cudaSuccess)
        {
            setFfiError(cudaGetErrorString(e));
            return -3;
        }
        return 0;
    }

    // ====================================================================
    // 异步帧管线（视频逐帧专用）：pinned 双缓冲 + 每槽 stream，
    // H2D / kernel / D2H 全异步 → 相邻帧拷贝与计算重叠（双缓冲/多槽流水）。
    // mode: 0=gainmap16（p=[hdrIntensity, gamma, peak]）
    //       1=transform16（p=[exposure, gamma, rAdj, gAdj, bAdj, peak]）
    // ====================================================================
    struct AsyncSlot
    {
        int ready;
        int w, h, n;
        cudaStream_t stream;
        unsigned char *hIn, *hOut; // pinned host buffers
        unsigned char *dIn, *dOut; // device buffers
    };
    static AsyncSlot g_slots[ASYNC_SLOTS] = {};

    /** 准备槽位（pinned 输入/输出 + 设备缓冲 + stream；尺寸变化时自动重建） */
    FFI_API int hdr_ffi_frame_prepare(int slot, int w, int h)
    {
        if (slot < 0 || slot >= ASYNC_SLOTS)
        {
            setFfiError("frame_prepare: bad slot");
            return -5;
        }
        AsyncSlot *s = &g_slots[slot];
        int n = w * h;
        if (s->ready && s->w == w && s->h == h)
            return 0;
        if (s->ready)
        {
            if (s->hIn) cudaFreeHost(s->hIn);
            if (s->hOut) cudaFreeHost(s->hOut);
            if (s->dIn) cudaFree(s->dIn);
            if (s->dOut) cudaFree(s->dOut);
            if (s->stream) cudaStreamDestroy(s->stream);
            s->ready = 0;
        }
        cudaError_t e = cudaHostAlloc(&s->hIn, (size_t)n * 4, cudaHostAllocDefault);
        if (e == cudaSuccess)
            e = cudaHostAlloc(&s->hOut, (size_t)n * 6, cudaHostAllocDefault);
        if (e == cudaSuccess)
            e = cudaMalloc(&s->dIn, (size_t)n * 4);
        if (e == cudaSuccess)
            e = cudaMalloc(&s->dOut, (size_t)n * 6);
        if (e == cudaSuccess)
            e = cudaStreamCreate(&s->stream);
        if (e != cudaSuccess)
        {
            setFfiError(cudaGetErrorString(e));
            return -3;
        }
        s->w = w;
        s->h = h;
        s->n = n;
        s->ready = 1;
        return 0;
    }

    /** 提交一帧（异步）：拷入 pinned → async H2D → kernel → async D2H */
    FFI_API int hdr_ffi_frame_submit(int slot, const unsigned char *rgba, int mode, const double *p, int np)
    {
        if (slot < 0 || slot >= ASYNC_SLOTS)
        {
            setFfiError("frame_submit: bad slot");
            return -5;
        }
        AsyncSlot *s = &g_slots[slot];
        if (!s->ready)
        {
            setFfiError("frame_submit: slot not prepared");
            return -5;
        }
        int n = s->n;
        memcpy(s->hIn, rgba, (size_t)n * 4); // host copy into pinned（async H2D 需要 pinned 源）
        cudaError_t e = cudaMemcpyAsync(s->dIn, s->hIn, (size_t)n * 4, cudaMemcpyHostToDevice, s->stream);
        if (e == cudaSuccess)
        {
            int blocks = numBlocks(n);
            if (mode == 0)
            {
                double maxBoostD = pow(2.0, p[0]);
                if (maxBoostD < 1.0) maxBoostD = 1.0;
                if (maxBoostD > 64.0) maxBoostD = 64.0;
                kFrameGainMap16<<<blocks, THREADS, 0, s->stream>>>(
                    (uchar4 *)s->dIn, s->dOut, n, maxBoostD, p[1], p[2]);
            }
            else
            {
                kFrameTransform16<<<blocks, THREADS, 0, s->stream>>>(
                    (uchar4 *)s->dIn, s->dOut, n, p[0], p[1], p[2], p[3], p[4], p[5]);
            }
            e = cudaGetLastError();
            if (e == cudaSuccess)
                e = cudaMemcpyAsync(s->hOut, s->dOut, (size_t)n * 6, cudaMemcpyDeviceToHost, s->stream);
        }
        if (e != cudaSuccess)
        {
            setFfiError(cudaGetErrorString(e));
            return -3;
        }
        return 0;
    }

    /** 等待槽位完成并把结果拷给调用方（同步点） */
    FFI_API int hdr_ffi_frame_wait(int slot, unsigned char *out)
    {
        if (slot < 0 || slot >= ASYNC_SLOTS)
        {
            setFfiError("frame_wait: bad slot");
            return -5;
        }
        AsyncSlot *s = &g_slots[slot];
        if (!s->ready)
        {
            setFfiError("frame_wait: slot not prepared");
            return -5;
        }
        cudaError_t e = cudaStreamSynchronize(s->stream);
        if (e == cudaSuccess)
            memcpy(out, s->hOut, (size_t)s->n * 6);
        if (e != cudaSuccess)
        {
            setFfiError(cudaGetErrorString(e));
            return -3;
        }
        return 0;
    }

    /** 释放槽位（在 hdr_ffi_cleanup 中调用） */
    FFI_API void hdr_ffi_frame_release(int slot)
    {
        if (slot < 0 || slot >= ASYNC_SLOTS)
            return;
        AsyncSlot *s = &g_slots[slot];
        if (!s->ready)
            return;
        if (s->stream) cudaStreamDestroy(s->stream);
        if (s->dOut) cudaFree(s->dOut);
        if (s->dIn) cudaFree(s->dIn);
        if (s->hOut) cudaFreeHost(s->hOut);
        if (s->hIn) cudaFreeHost(s->hIn);
        s->ready = 0;
        s->w = s->h = s->n = 0;
    }

    /** 异步槽位数（客户端可查，决定流水深度） */
    FFI_API int hdr_ffi_frame_slots()
    {
        return ASYNC_SLOTS;
    }

} // extern "C"