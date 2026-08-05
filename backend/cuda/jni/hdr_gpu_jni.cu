// ============================================================================
// hdr_gpu_jni.cu —— HDR Converter 的 CUDA 加速 JNI 桥
//
// 加速点（与 Kotlin 端 CPU 实现逐位对齐，保证输出完全一致）：
//   1. srgbRgbaToDisplayP3Rgba   主图像像素 sRGB -> Display-P3（sRGB 传递）
//   2. computeGainMap            高光扩展增益图（8-bit）+ min/max 归约
//   3. applyHdrTransform         PNG 路径的 HDR 色调映射（自动伽马 + 曝光 + 通道调整）
//
// 构建（见 build_jni.bat）：
//   nvcc -shared -O3 -gencode arch=compute_75,code=compute_75 -cudart=static \
//        -I"%JAVA_HOME%\include" -I"%JAVA_HOME%\include\win32" \
//        -Xcompiler "/MD" hdr_gpu_jni.cu -o hdr_gpu_jni.dll
//
// 说明：
//   - 使用 CUDA Runtime API（编译期链接 cudart，运行时仅依赖 nvcuda.dll 驱动）
//   - 内核以 compute_75 PTX 嵌入，驱动会为新 GPU JIT 编译，兼容性好
//   - GPU 初始化失败时 Kotlin 端自动回退 CPU，本 DLL 不影响应用可用性
// ============================================================================

#include <jni.h>
#include <cuda_runtime.h>
#include <vector_functions.h>
#include <cstring>
#include <cstdio>
#include <cmath>
#include <vector>

// ============================== 设备端工具 ==============================

__device__ __forceinline__ float dSrgbToLinear(float c)
{
    return c <= 0.04045f ? c / 12.92f : powf((c + 0.055f) / 1.055f, 2.4f);
}
__device__ __forceinline__ float dLinearToSrgb(float v)
{
    return v <= 0.0031308f ? v * 12.92f : 1.055f * powf(v, 1.0f / 2.4f) - 0.055f;
}
__device__ __forceinline__ float dClamp(float v, float lo, float hi)
{
    return fminf(hi, fmaxf(lo, v));
}

// ============================== 内核 1：sRGB -> Display-P3 ==============================
// 与 Kotlin srgbRgbaToDisplayP3Rgba 一致（矩阵 + sRGB 传递编码）
__global__ void kSrgbToP3(const uchar4 *__restrict__ in, uchar4 *__restrict__ out, int n)
{
    int i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i >= n)
        return;
    uchar4 p = in[i];
    float r = dSrgbToLinear(p.x / 255.0f);
    float g = dSrgbToLinear(p.y / 255.0f);
    float b = dSrgbToLinear(p.z / 255.0f);
    float pr = 0.82246f * r + 0.17749f * g + 0.00005f * b;
    float pg = 0.03311f * r + 0.96687f * g + 0.00002f * b;
    float pb = 0.01709f * r + 0.07239f * g + 0.91053f * b;
    out[i] = make_uchar4(
        (unsigned char)(int)(dLinearToSrgb(dClamp(pr, 0.0f, 1.0f)) * 255.0f + 0.5f),
        (unsigned char)(int)(dLinearToSrgb(dClamp(pg, 0.0f, 1.0f)) * 255.0f + 0.5f),
        (unsigned char)(int)(dLinearToSrgb(dClamp(pb, 0.0f, 1.0f)) * 255.0f + 0.5f),
        255);
}

// ============================== 内核 2a：增益图（计算 gain + 分块 min/max） ==============================
// 与 Kotlin computeGainMap 一致：
//   mask = clamp((y-0.25)/0.75, 0, 1)^gamma；gainPerPix = 1 + (maxBoost-1)*mask
//   pg = (y*gainPerPix + offset) / (y + offset)；offset = 1/64
__global__ void kGainMapPhase1(const uchar4 *__restrict__ in, float *__restrict__ gain,
                               float *__restrict__ partialMin, float *__restrict__ partialMax,
                               int n, float maxBoost, float gamma, float highlightStart, float offset)
{
    extern __shared__ float sdata[];
    int tid = threadIdx.x;
    int i = blockIdx.x * blockDim.x + tid;
    float lmin = 1e30f; // 越界线程用 ±大值，归约时不影响
    float lmax = -1e30f;
    float g = 1.0f;
    if (i < n)
    {
        uchar4 p = in[i];
        float r = dSrgbToLinear(p.x / 255.0f);
        float gg = dSrgbToLinear(p.y / 255.0f);
        float b = dSrgbToLinear(p.z / 255.0f);
        float y = 0.2126f * r + 0.7152f * gg + 0.0722f * b;
        float mask = powf(dClamp((y - highlightStart) / (1.0f - highlightStart), 0.0f, 1.0f), gamma);
        float gainPerPix = 1.0f + (maxBoost - 1.0f) * mask;
        float yhdr = y * gainPerPix;
        g = (yhdr + offset) / (y + offset);
        gain[i] = g;
        lmin = g;
        lmax = g;
    }
    // min 归约
    sdata[tid] = lmin;
    __syncthreads();
    for (int s = blockDim.x / 2; s > 0; s >>= 1)
    {
        if (tid < s)
            sdata[tid] = fminf(sdata[tid], sdata[tid + s]);
        __syncthreads();
    }
    if (tid == 0)
        partialMin[blockIdx.x] = sdata[0];
    // max 归约
    __syncthreads();
    sdata[tid] = lmax;
    __syncthreads();
    for (int s = blockDim.x / 2; s > 0; s >>= 1)
    {
        if (tid < s)
            sdata[tid] = fmaxf(sdata[tid], sdata[tid + s]);
        __syncthreads();
    }
    if (tid == 0)
        partialMax[blockIdx.x] = sdata[0];
}

// ============================== 内核 2b：增益图量化 ==============================
// logRec = (log2(gain) - mapMin) / range；gm8 = round(clamp(logRec,0,1)*255)
__global__ void kGainMapPhase2(const float *__restrict__ gain, unsigned char *__restrict__ out,
                               int n, float mapMin, float mapMax, float range)
{
    int i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i >= n)
        return;
    float logRec = (log2f(gain[i]) - mapMin) / range;
    float rec = dClamp(logRec, 0.0f, 1.0f);
    out[i] = (unsigned char)(int)(rec * 255.0f + 0.5f);
}

// ============================== 内核 3a：HDR 变换（sRGB -> 线性 + 亮度归约） ==============================
__global__ void kHdrTransformPhase1(const uchar4 *__restrict__ in, float3 *__restrict__ linear,
                                    float *__restrict__ partialLum, int n)
{
    extern __shared__ float sdata[];
    int tid = threadIdx.x;
    int i = blockIdx.x * blockDim.x + tid;
    float lum = 0.0f;
    if (i < n)
    {
        uchar4 p = in[i];
        float r = dSrgbToLinear(p.x / 255.0f);
        float g = dSrgbToLinear(p.y / 255.0f);
        float b = dSrgbToLinear(p.z / 255.0f);
        linear[i] = make_float3(r, g, b);
        lum = 0.2126f * r + 0.7152f * g + 0.0722f * b;
    }
    sdata[tid] = lum;
    __syncthreads();
    for (int s = blockDim.x / 2; s > 0; s >>= 1)
    {
        if (tid < s)
            sdata[tid] += sdata[tid + s];
        __syncthreads();
    }
    if (tid == 0)
        partialLum[blockIdx.x] = sdata[0];
}

// ============================== 内核 3b：HDR 变换（自动伽马 + 通道/曝光 + sRGB 编码） ==============================
__global__ void kHdrTransformPhase2(const float3 *__restrict__ linear, uchar4 *__restrict__ out,
                                    int n, float autoGamma, float totalExposure,
                                    float rAdj, float gAdj, float bAdj, float gamma)
{
    int i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i >= n)
        return;
    float3 v = linear[i];
    float r = v.x, g = v.y, b = v.z;
    if (autoGamma != 1.0f)
    {
        r = powf(dClamp(r, 0.0f, 3.4e38f), autoGamma);
        g = powf(dClamp(g, 0.0f, 3.4e38f), autoGamma);
        b = powf(dClamp(b, 0.0f, 3.4e38f), autoGamma);
    }
    r *= rAdj;
    g *= gAdj;
    b *= bAdj;
    r *= totalExposure;
    g *= totalExposure;
    b *= totalExposure;
    r = powf(dClamp(r, 0.0f, 3.4e38f), gamma);
    g = powf(dClamp(g, 0.0f, 3.4e38f), gamma);
    b = powf(dClamp(b, 0.0f, 3.4e38f), gamma);
    out[i] = make_uchar4(
        (unsigned char)(int)(dClamp(dLinearToSrgb(r), 0.0f, 1.0f) * 255.0f + 0.5f),
        (unsigned char)(int)(dClamp(dLinearToSrgb(g), 0.0f, 1.0f) * 255.0f + 0.5f),
        (unsigned char)(int)(dClamp(dLinearToSrgb(b), 0.0f, 1.0f) * 255.0f + 0.5f),
        255);
}

// ============================== 主机端 JNI ==============================

#define THREADS 256

static void throwJni(JNIEnv *env, const char *msg)
{
    jclass cls = env->FindClass("java/lang/RuntimeException");
    if (cls)
        env->ThrowNew(cls, msg);
    else
        fprintf(stderr, "[hdr_gpu_jni] %s\n", msg);
}

static inline int numBlocks(int n) { return (n + THREADS - 1) / THREADS; }

// ---- 初始化：探测 GPU ----
extern "C" JNIEXPORT jboolean JNICALL
Java_com_hdrconverter_HdrGpuJni_nativeInit(JNIEnv *env, jobject /*thiz*/)
{
    cudaError_t e = cudaSetDevice(0);
    if (e != cudaSuccess)
    {
        fprintf(stderr, "[hdr_gpu_jni] init failed: %s\n", cudaGetErrorString(e));
        return JNI_FALSE;
    }
    cudaDeviceProp prop;
    if (cudaGetDeviceProperties(&prop, 0) == cudaSuccess)
    {
        fprintf(stderr, "[hdr_gpu_jni] CUDA ready: %s (sm_%d%d, %.1f GB)\n",
                prop.name, prop.major, prop.minor, prop.totalGlobalMem / (1024.0 * 1024.0 * 1024.0));
    }
    return JNI_TRUE;
}

// ---- 释放（预留，当前上下文随进程销毁）----
extern "C" JNIEXPORT void JNICALL
Java_com_hdrconverter_HdrGpuJni_nativeRelease(JNIEnv * /*env*/, jobject /*thiz*/)
{
    cudaDeviceReset();
}

// ---- srgbRgbaToDisplayP3Rgba ----
extern "C" JNIEXPORT jboolean JNICALL
Java_com_hdrconverter_HdrGpuJni_nativeSrgbToDisplayP3(
    JNIEnv *env, jobject /*thiz*/, jbyteArray rgba, jint width, jint height, jbyteArray out)
{
    jsize n4 = env->GetArrayLength(rgba);
    int n = width * height;
    if (n4 != (jsize)(n * 4) || n <= 0)
    {
        throwJni(env, "srgbToDisplayP3: array size mismatch");
        return JNI_FALSE;
    }
    jbyte *src = env->GetByteArrayElements(rgba, nullptr);
    jbyte *dst = env->GetByteArrayElements(out, nullptr);
    if (!src || !dst)
        return JNI_FALSE;

    uchar4 *dIn = nullptr;
    uchar4 *dOut = nullptr;
    cudaError_t e = cudaMalloc(&dIn, (size_t)n * 4);
    if (e == cudaSuccess)
        e = cudaMalloc(&dOut, (size_t)n * 4);
    if (e == cudaSuccess)
        e = cudaMemcpy(dIn, src, (size_t)n * 4, cudaMemcpyHostToDevice);
    if (e == cudaSuccess)
    {
        kSrgbToP3<<<numBlocks(n), THREADS>>>(dIn, dOut, n);
        e = cudaGetLastError();
        if (e == cudaSuccess)
            e = cudaDeviceSynchronize();
    }
    if (e == cudaSuccess)
        e = cudaMemcpy(dst, dOut, (size_t)n * 4, cudaMemcpyDeviceToHost);

    if (dOut)
        cudaFree(dOut);
    if (dIn)
        cudaFree(dIn);
    env->ReleaseByteArrayElements(rgba, src, JNI_ABORT);
    env->ReleaseByteArrayElements(out, dst, 0);
    if (e != cudaSuccess)
    {
        throwJni(env, cudaGetErrorString(e));
        return JNI_FALSE;
    }
    return JNI_TRUE;
}

// ---- computeGainMap：outMinMax[0]=minBoost, [1]=maxBoostActual ----
extern "C" JNIEXPORT jboolean JNICALL
Java_com_hdrconverter_HdrGpuJni_nativeComputeGainMap(
    JNIEnv *env, jobject /*thiz*/, jbyteArray rgba, jint width, jint height,
    jdouble hdrIntensity, jdouble gamma, jbyteArray outGm8, jdoubleArray outMinMax)
{
    int n = width * height;
    jsize n4 = env->GetArrayLength(rgba);
    jsize nOut = env->GetArrayLength(outGm8);
    if (n4 != (jsize)(n * 4) || nOut != (jsize)n || n <= 0)
    {
        throwJni(env, "computeGainMap: array size mismatch");
        return JNI_FALSE;
    }

    jbyte *src = env->GetByteArrayElements(rgba, nullptr);
    jbyte *gm8 = env->GetByteArrayElements(outGm8, nullptr);
    jdouble *minmax = env->GetDoubleArrayElements(outMinMax, nullptr);
    if (!src || !gm8 || !minmax)
        return JNI_FALSE;

    double maxBoostD = pow(2.0, hdrIntensity);
    if (maxBoostD < 1.0)
        maxBoostD = 1.0;
    if (maxBoostD > 16.0)
        maxBoostD = 16.0;
    float maxBoost = (float)maxBoostD;
    float gammaF = (float)gamma;
    const float highlightStart = 0.25f;
    const float offset = 1.0f / 64.0f;

    int blocks = numBlocks(n);
    uchar4 *dIn = nullptr;
    float *dGain = nullptr;
    unsigned char *dGm8 = nullptr;
    float *dMin = nullptr;
    float *dMax = nullptr;

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
        e = cudaMemcpy(dIn, src, (size_t)n * 4, cudaMemcpyHostToDevice);
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
        minmax[0] = minBoost;
        minmax[1] = maxBoostActual;
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
    env->ReleaseByteArrayElements(rgba, src, JNI_ABORT);
    env->ReleaseByteArrayElements(outGm8, gm8, 0);
    env->ReleaseDoubleArrayElements(outMinMax, minmax, 0);
    if (e != cudaSuccess)
    {
        throwJni(env, cudaGetErrorString(e));
        return JNI_FALSE;
    }
    return JNI_TRUE;
}

// ---- applyHdrTransform（PNG 路径） ----
extern "C" JNIEXPORT jboolean JNICALL
Java_com_hdrconverter_HdrGpuJni_nativeApplyHdrTransform(
    JNIEnv *env, jobject /*thiz*/, jbyteArray rgba, jint width, jint height,
    jdouble totalExposure, jdouble gamma, jdouble rAdj, jdouble gAdj, jdouble bAdj,
    jbyteArray out)
{
    int n = width * height;
    jsize n4 = env->GetArrayLength(rgba);
    jsize nOut = env->GetArrayLength(out);
    if (n4 != (jsize)(n * 4) || nOut != (jsize)(n * 4) || n <= 0)
    {
        throwJni(env, "applyHdrTransform: array size mismatch");
        return JNI_FALSE;
    }
    jbyte *src = env->GetByteArrayElements(rgba, nullptr);
    jbyte *dst = env->GetByteArrayElements(out, nullptr);
    if (!src || !dst)
        return JNI_FALSE;

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
        e = cudaMemcpy(dIn, src, (size_t)n * 4, cudaMemcpyHostToDevice);
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
        e = cudaMemcpy(dst, dOut, (size_t)n * 4, cudaMemcpyDeviceToHost);

    if (dLum)
        cudaFree(dLum);
    if (dOut)
        cudaFree(dOut);
    if (dLinear)
        cudaFree(dLinear);
    if (dIn)
        cudaFree(dIn);
    env->ReleaseByteArrayElements(rgba, src, JNI_ABORT);
    env->ReleaseByteArrayElements(out, dst, 0);
    if (e != cudaSuccess)
    {
        throwJni(env, cudaGetErrorString(e));
        return JNI_FALSE;
    }
    return JNI_TRUE;
}
