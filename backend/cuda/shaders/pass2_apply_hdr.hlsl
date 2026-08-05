// Pass 2: 应用 HDR 处理并编码回 sRGB 8-bit
// 输入: float3 线性值缓冲区
// 输出: RGBA 8-bit 纹理

RWStructuredBuffer<float3> LinearInput : register(u0);
RWTexture2D<float4> OutputImage : register(u1);

cbuffer Constants : register(b0)
{
    uint ImageWidth;
    uint ImageHeight;
    uint TotalPixels;
    uint Pad0;

    float TotalExposure;  // totalExposure - 1
    float Gamma;          // 用户伽马
    float RAdj;
    float GAdj;
    float BAdj;
    float AutoGamma;      // 自动伽马值 (由 CPU 计算后传入)
    float UseAutoGamma;   // 0.0 或 1.0
    float2 Pad1;
};

// 线性 → sRGB 伽马编码
float linearToSrgb(float v)
{
    if (v <= 0.0031308)
        return v * 12.92;
    else
        return 1.055 * pow(v, 1.0 / 2.4) - 0.055;
}

// 钳制到 [0,1]
float clamp01(float v)
{
    return clamp(v, 0.0, 1.0);
}

[numthreads(16, 16, 1)]
void main(uint3 dtid : SV_DispatchThreadID)
{
    if (dtid.x >= ImageWidth || dtid.y >= ImageHeight)
        return;

    uint pixelIdx = dtid.y * ImageWidth + dtid.x;

    float3 linear = LinearInput[pixelIdx];

    // Pass 2: 自动伽马 (在 Pass1 的线性值上应用)
    if (UseAutoGamma > 0.5)
    {
        linear.r = pow(max(linear.r, 0.0), AutoGamma);
        linear.g = pow(max(linear.g, 0.0), AutoGamma);
        linear.b = pow(max(linear.b, 0.0), AutoGamma);
    }

    // Pass 3: RGB 通道调整 + 曝光 + 用户伽马 + sRGB 编码
    float r = linear.r * RAdj * TotalExposure;
    float g = linear.g * GAdj * TotalExposure;
    float b = linear.b * BAdj * TotalExposure;

    r = pow(max(r, 0.0), Gamma);
    g = pow(max(g, 0.0), Gamma);
    b = pow(max(b, 0.0), Gamma);

    r = linearToSrgb(r);
    g = linearToSrgb(g);
    b = linearToSrgb(b);

    OutputImage[dtid.xy] = float4(clamp01(r), clamp01(g), clamp01(b), 1.0);
}
