// CUDA Kernels for HDR Conversion
// Compile: nvcc -ptx cuda_kernels.cu -o cuda_kernels.ptx

// sRGB gamma decode
__device__ float srgbToLinear(float c)
{
    if (c <= 0.04045f)
        return c / 12.92f;
    else
        return powf((c + 0.055f) / 1.055f, 2.4f);
}

// Linear -> sRGB gamma encode
__device__ float linearToSrgb(float v)
{
    if (v <= 0.0031308f)
        return v * 12.92f;
    else
        return 1.055f * powf(v, 1.0f / 2.4f) - 0.055f;
}

// Pass 1: sRGB -> Linear + luminance reduction (per-block partial sum)
// input:  RGBA 8-bit (uchar4), output: float3, partialLum: per-block luminance sum
extern "C" __global__ void pass1_srgb_to_linear(
    const uchar4 *__restrict__ input,
    float3 *__restrict__ linearOutput,
    float *__restrict__ partialLuminance,
    int width, int height, int pitch)
{
    __shared__ float sharedLum[256];

    int x = blockIdx.x * blockDim.x + threadIdx.x;
    int y = blockIdx.y * blockDim.y + threadIdx.y;
    int tid = threadIdx.y * blockDim.x + threadIdx.x;

    if (x >= width || y >= height)
    {
        sharedLum[tid] = 0.0f;
        __syncthreads();
        if (tid == 0)
        {
            int groupIdx = blockIdx.y * gridDim.x + blockIdx.x;
            partialLuminance[groupIdx] = 0.0f;
        }
        return;
    }

    int pixelIdx = y * width + x;

    uchar4 p = input[pixelIdx];
    float r = p.x / 255.0f;
    float g = p.y / 255.0f;
    float b = p.z / 255.0f;

    float lr = srgbToLinear(r);
    float lg = srgbToLinear(g);
    float lb = srgbToLinear(b);

    linearOutput[pixelIdx] = make_float3(lr, lg, lb);

    float lum = 0.2126f * lr + 0.7152f * lg + 0.0722f * lb;
    sharedLum[tid] = lum;

    __syncthreads();

    // tree reduction
    for (int s = 128; s > 0; s >>= 1)
    {
        if (tid < s)
            sharedLum[tid] += sharedLum[tid + s];
        __syncthreads();
    }

    if (tid == 0)
    {
        int groupIdx = blockIdx.y * gridDim.x + blockIdx.x;
        partialLuminance[groupIdx] = sharedLum[0];
    }
}

// Pass 2: Apply HDR processing + sRGB encode
extern "C" __global__ void pass2_apply_hdr(
    const float3 *__restrict__ linearInput,
    uchar4 *__restrict__ output,
    int width, int height,
    float totalExposure,
    float gamma,
    float rAdj, float gAdj, float bAdj,
    float autoGamma,
    bool useAutoGamma)
{
    int x = blockIdx.x * blockDim.x + threadIdx.x;
    int y = blockIdx.y * blockDim.y + threadIdx.y;

    if (x >= width || y >= height)
        return;

    int pixelIdx = y * width + x;
    float3 linear = linearInput[pixelIdx];

    // Apply auto gamma
    if (useAutoGamma)
    {
        linear.x = powf(fmaxf(linear.x, 0.0f), autoGamma);
        linear.y = powf(fmaxf(linear.y, 0.0f), autoGamma);
        linear.z = powf(fmaxf(linear.z, 0.0f), autoGamma);
    }

    // Apply exposure, RGB adjust, user gamma, then sRGB encode
    float r = linear.x * rAdj * totalExposure;
    float g = linear.y * gAdj * totalExposure;
    float b = linear.z * bAdj * totalExposure;

    r = powf(fmaxf(r, 0.0f), gamma);
    g = powf(fmaxf(g, 0.0f), gamma);
    b = powf(fmaxf(b, 0.0f), gamma);

    r = linearToSrgb(r);
    g = linearToSrgb(g);
    b = linearToSrgb(b);

    auto clamp01 = [](float v) -> float
    { return fminf(fmaxf(v, 0.0f), 1.0f); };

    output[pixelIdx] = make_uchar4(
        (unsigned char)(clamp01(r) * 255.0f + 0.5f),
        (unsigned char)(clamp01(g) * 255.0f + 0.5f),
        (unsigned char)(clamp01(b) * 255.0f + 0.5f),
        255);
}
