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

    // ====================================================================
    //  图片 ICC 链路 v2（2026-10 与 CPU 同步）：明度 B 曲线 + 输出色域 + Oklab 色域拓展
    //
    //  与 Rust `convert::apply_hdr_rec2020_pq` 同式：
    //    sRGB→线性 → y_max 峰值锚定 → tone_gain(y, m, rolloff, imageGamma).min(m)
    //    → [色域拓展] → 输出色域矩阵 → PQ(whiteNits/10000)
    //  色域拓展查表由 Rust `convert::GamutExpandTable` 预计算（33×65 float）后上传，
    //  GPU 只做双线性查表 + Oklab 正反变换（CPU/GPU 共用同一份数据，避免两处各自建表）。
    // ====================================================================
    static Mat3 gamutMatrix(int g)
    {
        Mat3 m;
        if (g == 0) // 0 = BT.709 / sRGB（恒等，无需转换）
        {
            const float id[9] = {1.0f, 0.0f, 0.0f, 0.0f, 1.0f, 0.0f, 0.0f, 0.0f, 1.0f};
            for (int k = 0; k < 9; k++)
                m.m[k] = id[k];
        }
        else if (g == 1) // 1 = BT.709 → Display-P3
        {
            const float p3[9] = {0.822462f, 0.177538f, 0.0f,
                                 0.033195f, 0.966805f, 0.0f,
                                 0.017082f, 0.072397f, 0.910521f};
            for (int k = 0; k < 9; k++)
                m.m[k] = p3[k];
        }
        else // 2 = BT.709 → BT.2020
        {
            const float r2020[9] = {0.6274038959f, 0.3292830384f, 0.0433130642f,
                                    0.0690972894f, 0.9195403951f, 0.0113623156f,
                                    0.0163914389f, 0.0880133078f, 0.8955952528f};
            for (int k = 0; k < 9; k++)
                m.m[k] = r2020[k];
        }
        return m;
    }

    FFI_API int hdr_ffi_apply_hdr_rec2020_pq_v2(const unsigned char *rgba, int w, int h,
                                               double peakNits, double whiteNits,
                                               double rolloff, double imageGamma,
                                               int outGamut, double gamutExpand,
                                               const float *expandTable, int expandNL, int expandNH,
                                               unsigned char *out)
    {
        int n = w * h;
        if (n <= 0)
        {
            setFfiError("apply_hdr_rec2020_pq_v2: bad size");
            return -5;
        }
        int blocks = numBlocks(n);
        double exposure = peakNits / whiteNits;

        uchar4 *dIn = nullptr;
        float3 *dLinear = nullptr;
        uchar4 *dOut = nullptr;
        float *dMax = nullptr;
        float *dTable = nullptr;

        cudaError_t e = cudaMalloc(&dIn, (size_t)n * 4);
        if (e == cudaSuccess)
            e = cudaMalloc(&dLinear, (size_t)n * sizeof(float3));
        if (e == cudaSuccess)
            e = cudaMalloc(&dOut, (size_t)n * 4);
        if (e == cudaSuccess)
            e = cudaMalloc(&dMax, (size_t)blocks * sizeof(float));
        if (e == cudaSuccess && expandTable != nullptr && expandNL > 1 && expandNH > 1)
            e = cudaMalloc(&dTable, (size_t)expandNL * expandNH * sizeof(float));
        if (e == cudaSuccess)
            e = cudaMemcpy(dIn, rgba, (size_t)n * 4, cudaMemcpyHostToDevice);
        if (e == cudaSuccess && dTable)
            e = cudaMemcpy(dTable, expandTable, (size_t)expandNL * expandNH * sizeof(float),
                           cudaMemcpyHostToDevice);

        // 1) 线性化 + 画面最亮亮度（峰值锚定，与 CPU / Ultra HDR 同语义）
        if (e == cudaSuccess)
        {
            kImgLinearizeMax<<<blocks, THREADS, THREADS * sizeof(float)>>>(dIn, dLinear, dMax, n);
            e = cudaGetLastError();
            if (e == cudaSuccess)
                e = cudaDeviceSynchronize();
        }
        double yMax = 0.0;
        if (e == cudaSuccess)
        {
            std::vector<float> hMax(blocks);
            e = cudaMemcpy(hMax.data(), dMax, (size_t)blocks * sizeof(float), cudaMemcpyDeviceToHost);
            for (int b = 0; b < blocks && e == cudaSuccess; b++)
                if ((double)hMax[b] > yMax)
                    yMax = (double)hMax[b];
        }
        double m = (yMax > 0.5) ? (exposure / yMax) : exposure;
        if (m < 1.0)
            m = 1.0;

        // 2) 明度增益 → [色域拓展] → 输出色域 → PQ
        if (e == cudaSuccess)
        {
            kImgToneExpandPq<<<blocks, THREADS>>>(
                dLinear, dOut, n, (float)m, (float)rolloff, (float)imageGamma,
                gamutMatrix(outGamut), (float)gamutExpand,
                dTable, expandNL, expandNH, (float)(whiteNits / 10000.0));
            e = cudaGetLastError();
            if (e == cudaSuccess)
                e = cudaDeviceSynchronize();
        }
        if (e == cudaSuccess)
            e = cudaMemcpy(out, dOut, (size_t)n * 4, cudaMemcpyDeviceToHost);

        if (dTable)
            cudaFree(dTable);
        if (dMax)
            cudaFree(dMax);
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

    // ====================================================================
    //  Ultra HDR 增益图 v2（2026-10 与 CPU 同步）：B 曲线 + 局部镜面高光（top-hat）
    //
    //  与 Rust `ultra_hdr::compute_gain_map` 同式（输入为已下采样到目标分辨率的 RGBA）：
    //    y → y_max 峰值锚定 m → top-hat specular → gain=(t+offset)/(y+offset) → 8-bit 量化
    //  返回 outMinMax[0]=minContentBoost、[1]=maxContentBoost。
    // ====================================================================
    FFI_API int hdr_ffi_compute_gainmap_bcurve(const unsigned char *rgba, int w, int h,
                                               double peakNits, double whiteNits,
                                               double rolloff, double imageGamma,
                                               unsigned char *gm8, double *outMinMax)
    {
        int n = w * h;
        if (n <= 0)
        {
            setFfiError("compute_gainmap_bcurve: bad size");
            return -5;
        }
        int blocks = numBlocks(n);
        double exposure = peakNits / whiteNits;
        if (!(exposure >= 1.0))
            exposure = 1.0;

        uchar4 *dIn = nullptr;
        float *dY = nullptr, *dTmp = nullptr, *dEro = nullptr, *dOpe = nullptr,
              *dSpec = nullptr, *dGain = nullptr, *dMin = nullptr, *dMax = nullptr;
        int *dArg = nullptr;
        unsigned char *dGm8 = nullptr;

        cudaError_t e = cudaMalloc(&dIn, (size_t)n * 4);
        if (e == cudaSuccess)
            e = cudaMalloc(&dY, (size_t)n * sizeof(float));
        if (e == cudaSuccess)
            e = cudaMalloc(&dTmp, (size_t)n * sizeof(float));
        if (e == cudaSuccess)
            e = cudaMalloc(&dEro, (size_t)n * sizeof(float));
        if (e == cudaSuccess)
            e = cudaMalloc(&dOpe, (size_t)n * sizeof(float));
        if (e == cudaSuccess)
            e = cudaMalloc(&dSpec, (size_t)n * sizeof(float));
        if (e == cudaSuccess)
            e = cudaMalloc(&dGain, (size_t)n * sizeof(float));
        if (e == cudaSuccess)
            e = cudaMalloc(&dGm8, (size_t)n);
        if (e == cudaSuccess)
            e = cudaMalloc(&dMin, (size_t)blocks * sizeof(float));
        if (e == cudaSuccess)
            e = cudaMalloc(&dMax, (size_t)blocks * sizeof(float));
        if (e == cudaSuccess)
            e = cudaMalloc((void **)&dArg, (size_t)blocks * sizeof(int));
        if (e == cudaSuccess)
            e = cudaMemcpy(dIn, rgba, (size_t)n * 4, cudaMemcpyHostToDevice);

        // 1) 线性亮度 + 全局最亮点（索引用于「最亮点强制推到峰值」）
        if (e == cudaSuccess)
        {
            kLumMax<<<blocks, THREADS>>>(dIn, dY, dMax, dArg, n);
            e = cudaGetLastError();
            if (e == cudaSuccess)
                e = cudaDeviceSynchronize();
        }
        float yMax = 0.0f;
        int peakIdx = 0;
        if (e == cudaSuccess)
        {
            std::vector<float> hMax(blocks);
            std::vector<int> hArg(blocks);
            e = cudaMemcpy(hMax.data(), dMax, (size_t)blocks * sizeof(float), cudaMemcpyDeviceToHost);
            if (e == cudaSuccess)
                e = cudaMemcpy(hArg.data(), dArg, (size_t)blocks * sizeof(int), cudaMemcpyDeviceToHost);
            for (int b = 0; b < blocks && e == cudaSuccess; b++)
                if (hMax[b] >= yMax) // >= 与 Rust max_by 的「取最后一个最大值」一致
                {
                    yMax = hMax[b];
                    peakIdx = hArg[b];
                }
        }
        double ymaxD = (double)yMax;
        if (ymaxD < 1e-6)
            ymaxD = 1e-6;
        double m = (ymaxD > 0.5) ? (exposure / ymaxD) : exposure;
        if (m < 1.0)
            m = 1.0;

        // 2) 局部镜面高光：开运算（腐蚀→膨胀）top-hat，半径 = min(3, (min(w,h)-1)/2)，至少 1
        int r = SPEC_RADIUS;
        int half = ((w < h ? w : h) - 1) / 2;
        if (half < 0)
            half = 0;
        if (r > half)
            r = half;
        if (r < 1)
            r = 1;
        dim3 blk(32, 8);
        dim3 grid((w + 31) / 32, (h + 7) / 8);
        if (e == cudaSuccess)
        {
            kMorphH<<<grid, blk>>>(dY, dTmp, w, h, r, 0);
            kMorphV<<<grid, blk>>>(dTmp, dEro, w, h, r, 0);
            kMorphH<<<grid, blk>>>(dEro, dTmp, w, h, r, 1);
            kMorphV<<<grid, blk>>>(dTmp, dOpe, w, h, r, 1);
            kSpecular<<<blocks, THREADS>>>(dY, dOpe, dSpec, n, peakIdx);
            e = cudaGetLastError();
            if (e == cudaSuccess)
                e = cudaDeviceSynchronize();
        }

        // 3) 增益 + 分块 min/max
        if (e == cudaSuccess)
        {
            kGainB<<<blocks, THREADS, THREADS * sizeof(float)>>>(
                dY, dSpec, dGain, dMin, dMax, n,
                (float)m, (float)rolloff, (float)imageGamma, (float)exposure, (float)(1.0 / 64.0));
            e = cudaGetLastError();
            if (e == cudaSuccess)
                e = cudaDeviceSynchronize();
        }

        float gmin = 1e30f, gmax = -1e30f;
        if (e == cudaSuccess)
        {
            std::vector<float> hMin(blocks), hMax2(blocks);
            e = cudaMemcpy(hMin.data(), dMin, (size_t)blocks * sizeof(float), cudaMemcpyDeviceToHost);
            if (e == cudaSuccess)
                e = cudaMemcpy(hMax2.data(), dMax, (size_t)blocks * sizeof(float), cudaMemcpyDeviceToHost);
            for (int b = 0; b < blocks && e == cudaSuccess; b++)
            {
                if (hMin[b] < gmin)
                    gmin = hMin[b];
                if (hMax2[b] > gmax)
                    gmax = hMax2[b];
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

        if (dArg)
            cudaFree(dArg);
        if (dMax)
            cudaFree(dMax);
        if (dMin)
            cudaFree(dMin);
        if (dGm8)
            cudaFree(dGm8);
        if (dGain)
            cudaFree(dGain);
        if (dSpec)
            cudaFree(dSpec);
        if (dOpe)
            cudaFree(dOpe);
        if (dEro)
            cudaFree(dEro);
        if (dTmp)
            cudaFree(dTmp);
        if (dY)
            cudaFree(dY);
        if (dIn)
            cudaFree(dIn);
        if (e != cudaSuccess)
        {
            setFfiError(cudaGetErrorString(e));
            return -3;
        }
        return 0;
    }

    // ---- 逐帧单层变换：输出线性 16-bit 大端（视频统一链路：单层色调映射）----
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
    // 固定单层色调映射：p=[exposure, gamma, rAdj, gAdj, bAdj, peak]（transform16）。
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
            // fixed transform16 (mode param kept for old DLL ABI compat; only transform supported)
            if (mode != 1)
            {
                setFfiError("frame_submit: only transform16 supported (mode=1)");
                return -4;
            }
            kFrameTransform16<<<blocks, THREADS, 0, s->stream>>>(
                (uchar4 *)s->dIn, s->dOut, n, p[0], p[1], p[2], p[3], p[4], p[5]);
            if (e == cudaSuccess)
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