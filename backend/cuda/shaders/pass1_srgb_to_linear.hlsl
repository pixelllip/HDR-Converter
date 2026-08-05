// Pass 1: sRGB → Linear 转换 + 计算每组亮度部分和
// 输入: RGBA 8-bit 纹理
// 输出1: float3 线性值缓冲区 (RWStructuredBuffer<float3>)
// 输出2: 每线程组一个 float 亮度部分和 (RWStructuredBuffer<float>)

RWTexture2D<float4> InputImage : register(u0);
RWStructuredBuffer<float3> LinearOutput : register(u1);
RWStructuredBuffer<float> PartialLuminance : register(u2);

cbuffer Constants : register(b0)
{
    uint ImageWidth;
    uint ImageHeight;
    uint TotalPixels;
    uint Pad0;
};

groupshared float sharedLum[256];

// sRGB 伽马解码
float srgbToLinear(float c)
{
    if (c <= 0.04045)
        return c / 12.92;
    else
        return pow((c + 0.055) / 1.055, 2.4);
}

[numthreads(16, 16, 1)]
void main(uint3 dtid : SV_DispatchThreadID, uint gi : SV_GroupIndex, uint3 gid : SV_GroupID)
{
    if (dtid.x >= ImageWidth || dtid.y >= ImageHeight)
    {
        sharedLum[gi] = 0.0;
        GroupMemoryBarrierWithGroupSync();
        if (gi == 0)
        {
            uint groupIdx = gid.y * ((ImageWidth + 15) / 16) + gid.x;
            PartialLuminance[groupIdx] = 0.0;
        }
        return;
    }

    uint pixelIdx = dtid.y * ImageWidth + dtid.x;

    // 读取 RGBA 8-bit, 归一化到 [0,1]
    float4 pixel = InputImage[dtid.xy];
    float r = pixel.r;
    float g = pixel.g;
    float b = pixel.b;

    // sRGB → 线性
    float lr = srgbToLinear(r);
    float lg = srgbToLinear(g);
    float lb = srgbToLinear(b);

    // 写入线性缓冲区
    LinearOutput[pixelIdx] = float3(lr, lg, lb);

    // 计算亮度并累加到共享内存
    float lum = 0.2126 * lr + 0.7152 * lg + 0.0722 * lb;
    sharedLum[gi] = lum;

    GroupMemoryBarrierWithGroupSync();

    // 树形归约: 256 → 1
    for (uint s = 128; s > 0; s >>= 1)
    {
        if (gi < s)
            sharedLum[gi] += sharedLum[gi + s];
        GroupMemoryBarrierWithGroupSync();
    }

    // 线程 0 将本组部分和写入全局缓冲区
    if (gi == 0)
    {
        uint groupIdx = gid.y * ((ImageWidth + 15) / 16) + gid.x;
        PartialLuminance[groupIdx] = sharedLum[0];
    }
}
