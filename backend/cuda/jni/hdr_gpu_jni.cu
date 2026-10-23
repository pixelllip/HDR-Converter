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

#define THREADS 256

// ============================== 设备端工具 ==============================

__device__ __forceinline__ float dSrgbToLinear(float c)
{
    return c <= 0.04045f ? c / 12.92f : powf((c + 0.055f) / 1.055f, 2.4f);
}
// double 精度版：与 Kotlin srgbToLinear(Double) 逐位对齐（视频逐帧 16-bit PAM 用）
__device__ __forceinline__ double dSrgbToLinearD(double c)
{
    return c <= 0.04045 ? c / 12.92 : pow((c + 0.055) / 1.055, 2.4);
}
__device__ __forceinline__ float dLinearToSrgb(float v)
{
    return v <= 0.0031308f ? v * 12.92f : 1.055f * powf(v, 1.0f / 2.4f) - 0.055f;
}
__device__ __forceinline__ float dClamp(float v, float lo, float hi)
{
    return fminf(hi, fmaxf(lo, v));
}
__device__ __forceinline__ float dPqEncode(float l)
{
    // PQ OETF (SMPTE ST 2084), l normalized to 10000 nits (0..1)
    l = dClamp(l, 0.0f, 1.0f);
    const float m1 = 0.1593017578125f;
    const float m2 = 78.84375f;
    const float c1 = 0.8359375f;
    const float c2 = 18.8515625f;
    const float c3 = 18.6875f;
    float lm1 = powf(l, m1);
    return powf((c1 + c2 * lm1) / (1.0f + c3 * lm1), m2);
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
//   mask = clamp((y-0.5)*2, 0, 1)^gamma；gainPerPix = 1 + (maxBoost-1)*mask（0.5 以下 gain=1 保中间调）
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

// ============================== 内核 3c：HDR 变换 -> Rec.2020/PQ 编码 ==============================
// 与 Kotlin applyHdrTransformToRec2020Pq 一致：变换后 Rec.709 -> Rec.2020 -> PQ
__global__ void kHdrTransformPhase2Rec2020Pq(const float3 *__restrict__ linear, uchar4 *__restrict__ out,
                                             int n, float autoGamma, float totalExposure,
                                             float rAdj, float gAdj, float bAdj, float gamma, float pqScale)
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
    // Rec.709 -> Rec.2020 (BT.2087), same as video zscale pin=bt709 -> p=bt2020
    float r2020 = 0.6274038959f * r + 0.3292830384f * g + 0.0433130642f * b;
    float g2020 = 0.0690972894f * r + 0.9195403951f * g + 0.0113623156f * b;
    float b2020 = 0.0163914389f * r + 0.0880133078f * g + 0.8955952528f * b;
    // PQ encode: L = linear * whiteNits/10000
    out[i] = make_uchar4(
        (unsigned char)(int)(dPqEncode(r2020 * pqScale) * 255.0f + 0.5f),
        (unsigned char)(int)(dPqEncode(g2020 * pqScale) * 255.0f + 0.5f),
        (unsigned char)(int)(dPqEncode(b2020 * pqScale) * 255.0f + 0.5f),
        255);
}

// ============================== 内核 4b：逐帧单层变换（输出线性 16-bit PAM） ==============================
// 与 Kotlin UltraHdrEncoder.reconstructLinearHdrTransform 逐位对齐（double 精度）：
//   r = srgbToLinear(p/255)*rAdj*exposure;  r = pow(fmax(r,0), gamma)   （同 g,b；exposure=peak）
//   输出 = round(clamp(ch, 0, peak)/peak*65535) → 16-bit 大端两字节
__global__ void kFrameTransform16(const uchar4 *__restrict__ in, unsigned char *__restrict__ out,
                                  int n, double exposure, double gamma,
                                  double rAdj, double gAdj, double bAdj, double peak)
{
    int i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i >= n)
        return;
    uchar4 p = in[i];
    double r = pow(fmax(0.0, dSrgbToLinearD(p.x / 255.0) * rAdj * exposure), gamma);
    double g = pow(fmax(0.0, dSrgbToLinearD(p.y / 255.0) * gAdj * exposure), gamma);
    double b = pow(fmax(0.0, dSrgbToLinearD(p.z / 255.0) * bAdj * exposure), gamma);
    int o = i * 6;
    int vr = (int)llround(fmin(peak, fmax(0.0, r)) / peak * 65535.0);
    int vg = (int)llround(fmin(peak, fmax(0.0, g)) / peak * 65535.0);
    int vb = (int)llround(fmin(peak, fmax(0.0, b)) / peak * 65535.0);
    out[o]     = (unsigned char)((vr >> 8) & 0xFF);
    out[o + 1] = (unsigned char)(vr & 0xFF);
    out[o + 2] = (unsigned char)((vg >> 8) & 0xFF);
    out[o + 3] = (unsigned char)(vg & 0xFF);
    out[o + 4] = (unsigned char)((vb >> 8) & 0xFF);
    out[o + 5] = (unsigned char)(vb & 0xFF);
}

// ============================================================================
//  图片 ICC 链路（2026-10 与 CPU 同步）：明度 B 曲线 + 输出色域 + Oklab 色域拓展
//
//  与 Rust 侧同式（单一事实来源在 Rust）：
//    `ultra_hdr::tone_target` / `tone_gain` —— 中调抬升 + 白点滚降 + 亮度域伽马
//    `convert::apply_hdr_rec2020_pq`        —— 明度增益 → 色域拓展 → 输出色域 → PQ
//  精度：float32（CPU 为 float64），8-bit 输出允许 ±1。
// ============================================================================

__device__ __forceinline__ float dSmoothstep(float a, float b, float x)
{
    float t = dClamp((x - a) / (b - a), 0.0f, 1.0f);
    return t * t * (3.0f - 2.0f * t);
}

/// 共用 tone 曲线（与 Rust `ultra_hdr::tone_target` 同式）：
/// `y^gamma` → 中调峰值 0.8M、白点 `rolloff*M` 滚降；m ≤ 1 时恒等（仅伽马）。
__device__ __forceinline__ float dToneTarget(float y, float m, float rolloff, float gamma)
{
    // 参数裁剪与 Rust `tone_target` 一致（gamma 0.1~2、rolloff 0~1）
    gamma = dClamp(gamma, 0.1f, 2.0f);
    rolloff = dClamp(rolloff, 0.0f, 1.0f);
    y = powf(dClamp(y, 0.0f, 1.0f), gamma);
    if (m <= 1.0f)
        return y;
    const float PEAK_Y = 0.30f;   // ROLLOFF_PEAK_Y
    const float PEAK_FRAC = 0.80f; // ROLLOFF_PEAK_FRAC
    float tPeak = PEAK_Y * (1.0f + (m - 1.0f) * PEAK_FRAC);
    if (y <= PEAK_Y)
        return y * (1.0f + (m - 1.0f) * PEAK_FRAC * dSmoothstep(0.0f, PEAK_Y, y));
    return tPeak + ((1.0f + (m - 1.0f) * rolloff) - tPeak) * dSmoothstep(PEAK_Y, 1.0f, y);
}

/// `tone_target / y`（分母用原 y；y ≤ 0 时 1.0）
__device__ __forceinline__ float dToneGain(float y, float m, float rolloff, float gamma)
{
    return y <= 0.0f ? 1.0f : dToneTarget(y, m, rolloff, gamma) / y;
}

// ---- Oklab（Björn Ottosson 2020），与 convert.rs 同常量 ----
__device__ __forceinline__ float3 dOklabFromLinearSrgb(float r, float g, float b)
{
    float l = 0.4122214708f * r + 0.5363325363f * g + 0.0514459929f * b;
    float m = 0.2119034982f * r + 0.6806995451f * g + 0.1073969566f * b;
    float s = 0.0883024619f * r + 0.2817188376f * g + 0.6299787005f * b;
    float l_ = cbrtf(l), m_ = cbrtf(m), s_ = cbrtf(s);
    return make_float3(
        0.2104542553f * l_ + 0.7936177850f * m_ - 0.0040720468f * s_,
        1.9779984951f * l_ - 2.4285922050f * m_ + 0.4505937099f * s_,
        0.0259040371f * l_ + 0.7827717662f * m_ - 0.8086757660f * s_);
}

__device__ __forceinline__ float3 dOklabToLinearSrgb(float l, float a, float b)
{
    float l_ = l + 0.3963377774f * a + 0.2158037573f * b;
    float m_ = l - 0.1055613458f * a - 0.0638541728f * b;
    float s_ = l - 0.0894841775f * a - 1.2914855480f * b;
    float l3 = l_ * l_ * l_, m3 = m_ * m_ * m_, s3 = s_ * s_ * s_;
    return make_float3(
        4.0767416621f * l3 - 3.3077115913f * m3 + 0.2309699292f * s3,
        -1.2684380046f * l3 + 2.6097574011f * m3 - 0.3413193965f * s3,
        -0.0041960863f * l3 - 0.7034186147f * m3 + 1.7076147010f * s3);
}

/// 色域拓展查表（33×65，行=明度、列=色相；h 环形闭合，双线性插值）。
/// 表由 Rust `convert::GamutExpandTable` 预计算并上传（CPU/GPU 共用同一份数据）。
__device__ __forceinline__ float dExpandRatio(const float *__restrict__ table, int nL, int nH,
                                              float l, float h)
{
    float x = dClamp(l, 0.0f, 1.0f) * (float)(nL - 1);
    int i0 = (int)floorf(x);
    if (i0 > nL - 1)
        i0 = nL - 1;
    if (i0 < 0)
        i0 = 0;
    int i1 = i0 + 1 < nL ? i0 + 1 : nL - 1;
    float fx = x - (float)i0;

    const float TAU = 6.283185307179586f;
    float t = (h / TAU);
    t = t - floorf(t); // rem_euclid(1)
    t *= (float)nH;
    float tf = floorf(t);
    int j0 = ((int)tf) % nH;
    if (j0 < 0)
        j0 += nH;
    int j1 = (j0 + 1) % nH;
    float fy = t - tf;

    float a = table[i0 * nH + j0] * (1.0f - fy) + table[i0 * nH + j1] * fy;
    float b = table[i1 * nH + j0] * (1.0f - fy) + table[i1 * nH + j1] * fy;
    return a * (1.0f - fx) + b * fx;
}

/// 线性 BT.709 → 输出色域矩阵（0=BT.709 恒等、1=Display-P3、2=BT.2020）
struct Mat3
{
    float m[9];
};

/// 第一遍：sRGB 码值 → 线性（BT.709）+ 分块最大亮度（供峰值锚定 m = 曝光 / y_max）
__global__ void kImgLinearizeMax(const uchar4 *__restrict__ in, float3 *__restrict__ linear,
                                 float *__restrict__ partialMax, int n)
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
            sdata[tid] = fmaxf(sdata[tid], sdata[tid + s]);
        __syncthreads();
    }
    if (tid == 0)
        partialMax[blockIdx.x] = sdata[0];
}

/// 第二遍：明度 B 曲线增益（`.min(toneM)` 与增益图侧对齐）→ 可选 Oklab 色域拓展
///        → 输出色域矩阵 → PQ 编码（scale = whiteNits/10000）。
__global__ void kImgToneExpandPq(const float3 *__restrict__ linear, uchar4 *__restrict__ out, int n,
                                 float toneM, float rolloff, float imageGamma,
                                 Mat3 gm, float expandStrength,
                                 const float *__restrict__ table, int nL, int nH, float pqScale)
{
    int i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i >= n)
        return;
    float3 v = linear[i];
    float r = v.x, g = v.y, b = v.z;
    float y = 0.2126f * r + 0.7152f * g + 0.0722f * b;
    float gain = fminf(dToneGain(y, toneM, rolloff, imageGamma), toneM);
    r = fmaxf(r * gain, 0.0f);
    g = fmaxf(g * gain, 0.0f);
    b = fmaxf(b * gain, 0.0f);

    // 色域拓展：归一化到最大分量 → Oklab 保持色相/明度、按边界比拉伸彩度 → 还原
    if (table != nullptr && expandStrength > 0.0f)
    {
        float mx = fmaxf(r, fmaxf(g, b));
        if (mx > 1e-9f)
        {
            float3 lab = dOklabFromLinearSrgb(r / mx, g / mx, b / mx);
            float c = sqrtf(lab.y * lab.y + lab.z * lab.z);
            if (c > 1e-6f)
            {
                float k = 1.0f + expandStrength * (dExpandRatio(table, nL, nH, lab.x, atan2f(lab.z, lab.y)) - 1.0f);
                float3 e = dOklabToLinearSrgb(lab.x, lab.y * k, lab.z * k);
                r = e.x * mx;
                g = e.y * mx;
                b = e.z * mx;
            }
        }
    }

    float ro = gm.m[0] * r + gm.m[1] * g + gm.m[2] * b;
    float go = gm.m[3] * r + gm.m[4] * g + gm.m[5] * b;
    float bo = gm.m[6] * r + gm.m[7] * g + gm.m[8] * b;
    out[i] = make_uchar4(
        (unsigned char)(int)(dPqEncode(ro * pqScale) * 255.0f + 0.5f),
        (unsigned char)(int)(dPqEncode(go * pqScale) * 255.0f + 0.5f),
        (unsigned char)(int)(dPqEncode(bo * pqScale) * 255.0f + 0.5f),
        255);
}

// ============================================================================
//  Ultra HDR 增益图（2026-10 与 CPU 同步）：B 曲线 + 局部镜面高光（top-hat）
//  与 Rust `ultra_hdr::compute_gain_map` 同式；输入为已下采样到目标分辨率的 RGBA。
// ============================================================================

#define SPEC_RADIUS 3 // ROLLOFF_SPEC_RADIUS
#define GATE_LO 0.70f
#define GATE_HI 0.95f

/// 线性亮度 + 分块最大亮度/索引（索引用于「最亮点强制推到峰值」的补丁）
__global__ void kLumMax(const uchar4 *__restrict__ in, float *__restrict__ y,
                        float *__restrict__ partialMax, int *__restrict__ partialArg, int n)
{
    __shared__ float sval[THREADS];
    __shared__ int sidx[THREADS];
    int tid = threadIdx.x;
    int i = blockIdx.x * blockDim.x + tid;
    float lv = -1e30f;
    int li = -1;
    if (i < n)
    {
        uchar4 p = in[i];
        float r = dSrgbToLinear(p.x / 255.0f);
        float g = dSrgbToLinear(p.y / 255.0f);
        float b = dSrgbToLinear(p.z / 255.0f);
        lv = 0.2126f * r + 0.7152f * g + 0.0722f * b;
        y[i] = lv;
        li = i;
    }
    sval[tid] = lv;
    sidx[tid] = li;
    __syncthreads();
    for (int s = blockDim.x / 2; s > 0; s >>= 1)
    {
        if (tid < s)
        {
            // ">=" 保留最后一个最大值，与 Rust `Iterator::max_by` 的取索引语义一致
            if (sval[tid + s] >= sval[tid])
            {
                sval[tid] = sval[tid + s];
                sidx[tid] = sidx[tid + s];
            }
        }
        __syncthreads();
    }
    if (tid == 0)
    {
        partialMax[blockIdx.x] = sval[0];
        partialArg[blockIdx.x] = sidx[0];
    }
}

/// 可分离形态学滤波（横向）：半径 r 的 min（腐蚀）/ max（膨胀）
__global__ void kMorphH(const float *__restrict__ src, float *__restrict__ dst,
                        int w, int h, int r, int wantMax)
{
    int x = blockIdx.x * blockDim.x + threadIdx.x;
    int y = blockIdx.y * blockDim.y + threadIdx.y;
    if (x >= w || y >= h)
        return;
    int lo = x - r < 0 ? 0 : x - r;
    int hi = x + r > w - 1 ? w - 1 : x + r;
    const float *row = src + (size_t)y * w;
    float v = row[lo];
    for (int i = lo + 1; i <= hi; i++)
        v = wantMax ? fmaxf(v, row[i]) : fminf(v, row[i]);
    dst[(size_t)y * w + x] = v;
}

/// 可分离形态学滤波（纵向）
__global__ void kMorphV(const float *__restrict__ src, float *__restrict__ dst,
                        int w, int h, int r, int wantMax)
{
    int x = blockIdx.x * blockDim.x + threadIdx.x;
    int y = blockIdx.y * blockDim.y + threadIdx.y;
    if (x >= w || y >= h)
        return;
    int lo = y - r < 0 ? 0 : y - r;
    int hi = y + r > h - 1 ? h - 1 : y + r;
    float v = src[(size_t)lo * w + x];
    for (int i = lo + 1; i <= hi; i++)
    {
        float z = src[(size_t)i * w + x];
        v = wantMax ? fmaxf(v, z) : fminf(v, z);
    }
    dst[(size_t)y * w + x] = v;
}

/// 局部镜面高光权重：top-hat（开运算）区分小面积亮斑与大面积白；
/// 最亮点（peakIdx）若确实有局部高光成分则强制推到峰值。
__global__ void kSpecular(const float *__restrict__ y, const float *__restrict__ opened,
                          float *__restrict__ spec, int n, int peakIdx)
{
    int i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i >= n)
        return;
    float yv = y[i], ov = opened[i];
    float s = dClamp((yv - ov) / fmaxf(1.0f - ov, 1e-3f), 0.0f, 1.0f);
    s = dClamp(s * dSmoothstep(GATE_LO, GATE_HI, yv), 0.0f, 1.0f);
    if (i == peakIdx && yv > 0.5f && s > 0.05f)
        s = 1.0f;
    spec[i] = s;
}

/// 增益：全局 tone_gain 与峰值按 spec 插值 → clamp[1, m] → (t+offset)/(y+offset)，分块 min/max
__global__ void kGainB(const float *__restrict__ y, const float *__restrict__ spec,
                       float *__restrict__ gain, float *__restrict__ partialMin,
                       float *__restrict__ partialMax, int n,
                       float mEff, float rolloff, float imageGamma, float exposure, float offset)
{
    extern __shared__ float sdata[];
    int tid = threadIdx.x;
    int i = blockIdx.x * blockDim.x + tid;
    float lmin = 1e30f, lmax = -1e30f, gn = 1.0f;
    if (i < n)
    {
        float yv = y[i], sp = spec[i];
        float gt = dToneGain(yv, mEff, rolloff, imageGamma);
        float g = fminf(fmaxf(gt + (mEff - gt) * sp, 1.0f), mEff);
        float t = fminf(yv * g, exposure);
        gn = (t + offset) / (yv + offset);
        gain[i] = gn;
        lmin = gn;
        lmax = gn;
    }
    // min
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
    // max
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

// ============================== 主机端 JNI ==============================

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
    if (maxBoostD > 64.0)
        maxBoostD = 64.0;
    float maxBoost = (float)maxBoostD;
    float gammaF = (float)gamma;
    const float highlightStart = 0.5f;
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

// ---- applyHdrTransformToRec2020Pq（png/jpg_icc 路径：Rec.709 -> Rec.2020 -> PQ 编码） ----
extern "C" JNIEXPORT jboolean JNICALL
Java_com_hdrconverter_HdrGpuJni_nativeApplyHdrTransformToRec2020Pq(
    JNIEnv *env, jobject /*thiz*/, jbyteArray rgba, jint width, jint height,
    jdouble totalExposure, jdouble gamma, jdouble rAdj, jdouble gAdj, jdouble bAdj,
    jdouble whiteNits, jbyteArray out)
{
    int n = width * height;
    jsize n4 = env->GetArrayLength(rgba);
    jsize nOut = env->GetArrayLength(out);
    if (n4 != (jsize)(n * 4) || nOut != (jsize)(n * 4) || n <= 0)
    {
        throwJni(env, "applyHdrTransformToRec2020Pq: array size mismatch");
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

    // auto gamma disabled: matches video preview (no auto gamma), avoids washed-out bright images
    float autoGamma = 1.0f;

    if (e == cudaSuccess)
    {
        float pqScale = (float)(whiteNits / 10000.0);
        kHdrTransformPhase2Rec2020Pq<<<blocks, THREADS>>>(
            dLinear, dOut, n, autoGamma, (float)totalExposure,
            (float)rAdj, (float)gAdj, (float)bAdj, (float)gamma, pqScale);
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

// ---- 逐帧单层变换：输出线性 16-bit PAM 大端字节（视频统一链路：单层色调映射） ----
extern "C" JNIEXPORT jboolean JNICALL
Java_com_hdrconverter_HdrGpuJni_nativeReconstructFrameTransform16(
    JNIEnv *env, jobject /*thiz*/, jbyteArray rgba, jint width, jint height,
    jdouble exposure, jdouble gamma, jdouble rAdj, jdouble gAdj, jdouble bAdj,
    jdouble peak, jbyteArray out)
{
    int n = width * height;
    jsize n4 = env->GetArrayLength(rgba);
    jsize nOut = env->GetArrayLength(out);
    if (n4 != (jsize)(n * 4) || nOut != (jsize)(n * 6) || n <= 0)
    {
        throwJni(env, "reconstructFrameTransform16: array size mismatch");
        return JNI_FALSE;
    }
    jbyte *src = env->GetByteArrayElements(rgba, nullptr);
    jbyte *dst = env->GetByteArrayElements(out, nullptr);
    if (!src || !dst)
        return JNI_FALSE;

    int blocks = numBlocks(n);
    uchar4 *dIn = nullptr;
    unsigned char *dOut = nullptr;
    cudaError_t e = cudaMalloc(&dIn, (size_t)n * 4);
    if (e == cudaSuccess)
        e = cudaMalloc(&dOut, (size_t)n * 6);
    if (e == cudaSuccess)
        e = cudaMemcpy(dIn, src, (size_t)n * 4, cudaMemcpyHostToDevice);
    if (e == cudaSuccess)
    {
        kFrameTransform16<<<blocks, THREADS>>>(dIn, dOut, n, exposure, gamma, rAdj, gAdj, bAdj, peak);
        e = cudaGetLastError();
        if (e == cudaSuccess)
            e = cudaDeviceSynchronize();
    }
    if (e == cudaSuccess)
        e = cudaMemcpy(dst, dOut, (size_t)n * 6, cudaMemcpyDeviceToHost);

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
