#include "directcompute_backend.h"
#include <windows.h>
#include <d3d11.h>
#include <d3dcompiler.h>
#include <vector>
#include <string>
#include <cstring>
#include <cmath>

#pragma comment(lib, "d3d11.lib")
#pragma comment(lib, "d3dcompiler.lib")

// ===== 内部状态 =====
static ID3D11Device *g_device = nullptr;
static ID3D11DeviceContext *g_context = nullptr;
static ID3D11ComputeShader *g_shaderPass1 = nullptr;
static ID3D11ComputeShader *g_shaderPass2 = nullptr;
static ID3D11Buffer *g_constantBuffer = nullptr;

// 输入纹理 (SRV) 和输出纹理 (UAV)
static ID3D11Texture2D *g_inputTexture = nullptr;
static ID3D11ShaderResourceView *g_inputSRV = nullptr;
static ID3D11Texture2D *g_outputTexture = nullptr;
static ID3D11UnorderedAccessView *g_outputUAV = nullptr;

// 线性缓冲区 (UAV)
static ID3D11Buffer *g_linearBuffer = nullptr;
static ID3D11UnorderedAccessView *g_linearUAV = nullptr;
static ID3D11ShaderResourceView *g_linearSRV = nullptr;

// 亮度部分和 (UAV + Staging for readback)
static ID3D11Buffer *g_lumBuffer = nullptr;
static ID3D11UnorderedAccessView *g_lumUAV = nullptr;
static ID3D11Buffer *g_lumStaging = nullptr;

// 当前图像尺寸
static int g_curWidth = 0;
static int g_curHeight = 0;
static int g_maxGroupsX = 0;
static int g_maxGroupsY = 0;

static std::string g_lastError;

// ===== 着色器字节码 (embedded) =====
// 这些由 compile_shaders.bat 生成并嵌入
// 如果着色器文件不存在, 使用内联编译

static const char *g_pass1CS = nullptr;
static const char *g_pass2CS = nullptr;
static size_t g_pass1Size = 0;
static size_t g_pass2Size = 0;

void dc_set_shader_bytecode(const void *pass1, size_t pass1Size,
                            const void *pass2, size_t pass2Size)
{
    g_pass1CS = (const char *)pass1;
    g_pass1Size = pass1Size;
    g_pass2CS = (const char *)pass2;
    g_pass2Size = pass2Size;
}

// ===== 内联编译 HLSL (后备方案) =====
static ID3DBlob *compile_hlsl(const char *source, const char *entryPoint)
{
    ID3DBlob *shaderBlob = nullptr;
    ID3DBlob *errorBlob = nullptr;

    UINT flags = D3DCOMPILE_ENABLE_STRICTNESS;
#ifdef _DEBUG
    flags |= D3DCOMPILE_DEBUG;
#endif

    HRESULT hr = D3DCompile(source, strlen(source), nullptr, nullptr, nullptr,
                            entryPoint, "cs_5_0", flags, 0, &shaderBlob, &errorBlob);
    if (FAILED(hr))
    {
        if (errorBlob)
        {
            g_lastError = "HLSL compile error: ";
            g_lastError += (const char *)errorBlob->GetBufferPointer();
            errorBlob->Release();
        }
        else
        {
            g_lastError = "D3DCompile failed with unknown error";
        }
        return nullptr;
    }
    if (errorBlob)
        errorBlob->Release();
    return shaderBlob;
}

// ===== Pass 1 HLSL source (fallback) =====
static const char *g_pass1Source = R"(
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

float srgbToLinear(float c)
{
    if (c <= 0.04045) return c / 12.92;
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
    float4 pixel = InputImage[dtid.xy];

    float lr = srgbToLinear(pixel.r);
    float lg = srgbToLinear(pixel.g);
    float lb = srgbToLinear(pixel.b);

    LinearOutput[pixelIdx] = float3(lr, lg, lb);

    float lum = 0.2126 * lr + 0.7152 * lg + 0.0722 * lb;
    sharedLum[gi] = lum;

    GroupMemoryBarrierWithGroupSync();

    for (uint s = 128; s > 0; s >>= 1)
    {
        if (gi < s) sharedLum[gi] += sharedLum[gi + s];
        GroupMemoryBarrierWithGroupSync();
    }

    if (gi == 0)
    {
        uint groupIdx = gid.y * ((ImageWidth + 15) / 16) + gid.x;
        PartialLuminance[groupIdx] = sharedLum[0];
    }
}
)";

// ===== Pass 2 HLSL source (fallback) =====
static const char *g_pass2Source = R"(
RWStructuredBuffer<float3> LinearInput : register(u0);
RWTexture2D<float4> OutputImage : register(u1);

cbuffer Constants : register(b0)
{
    uint ImageWidth;
    uint ImageHeight;
    uint TotalPixels;
    uint Pad0;
    float TotalExposure;
    float Gamma;
    float RAdj;
    float GAdj;
    float BAdj;
    float AutoGamma;
    float UseAutoGamma;
    float2 Pad1;
};

float linearToSrgb(float v)
{
    if (v <= 0.0031308) return v * 12.92;
    return 1.055 * pow(v, 1.0 / 2.4) - 0.055;
}

float clamp01(float v) { return clamp(v, 0.0, 1.0); }

[numthreads(16, 16, 1)]
void main(uint3 dtid : SV_DispatchThreadID)
{
    if (dtid.x >= ImageWidth || dtid.y >= ImageHeight) return;

    uint pixelIdx = dtid.y * ImageWidth + dtid.x;
    float3 linear = LinearInput[pixelIdx];

    if (UseAutoGamma > 0.5)
    {
        linear.r = pow(max(linear.r, 0.0), AutoGamma);
        linear.g = pow(max(linear.g, 0.0), AutoGamma);
        linear.b = pow(max(linear.b, 0.0), AutoGamma);
    }

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
)";

// ===== 辅助: 创建结构化缓冲区 =====
static ID3D11Buffer *create_structured_buffer(UINT stride, UINT count,
                                              const void *initData, D3D11_BIND_FLAG bindFlag,
                                              bool cpuRead = false)
{
    D3D11_BUFFER_DESC desc = {};
    desc.ByteWidth = stride * count;
    desc.StructureByteStride = stride;
    desc.BindFlags = bindFlag;
    desc.MiscFlags = D3D11_RESOURCE_MISC_BUFFER_STRUCTURED;
    desc.Usage = cpuRead ? D3D11_USAGE_STAGING : D3D11_USAGE_DEFAULT;
    desc.CPUAccessFlags = cpuRead ? D3D11_CPU_ACCESS_READ : 0;

    if (initData)
    {
        D3D11_SUBRESOURCE_DATA init = {};
        init.pSysMem = initData;
        ID3D11Buffer *buf = nullptr;
        HRESULT hr = g_device->CreateBuffer(&desc, &init, &buf);
        if (FAILED(hr))
        {
            g_lastError = "CreateBuffer failed";
            return nullptr;
        }
        return buf;
    }

    ID3D11Buffer *buf = nullptr;
    HRESULT hr = g_device->CreateBuffer(&desc, nullptr, &buf);
    if (FAILED(hr))
    {
        g_lastError = "CreateBuffer failed (no init)";
        return nullptr;
    }
    return buf;
}

// ===== 初始化 =====
bool dc_initialize()
{
    if (g_device)
        return true; // 已初始化

    g_lastError.clear();

    // 创建 D3D11 设备 (Compute Shader 需要 D3D_FEATURE_LEVEL_11_0)
    UINT flags = D3D11_CREATE_DEVICE_SINGLETHREADED;
#ifdef _DEBUG
    flags |= D3D11_CREATE_DEVICE_DEBUG;
#endif

    D3D_FEATURE_LEVEL featureLevels[] = {D3D_FEATURE_LEVEL_11_0};
    D3D_FEATURE_LEVEL outLevel;

    HRESULT hr = D3D11CreateDevice(
        nullptr, // 使用默认适配器
        D3D_DRIVER_TYPE_HARDWARE,
        nullptr, // 不加载软件驱动
        flags,
        featureLevels, 1,
        D3D11_SDK_VERSION,
        &g_device,
        &outLevel,
        &g_context);

    if (FAILED(hr))
    {
        g_lastError = "D3D11CreateDevice failed: ";
        g_lastError += std::to_string(hr);
        return false;
    }

    // 编译着色器 (使用内联源码)
    auto *blob1 = compile_hlsl(g_pass1Source, "main");
    if (!blob1)
    {
        dc_cleanup();
        return false;
    }
    hr = g_device->CreateComputeShader(
        blob1->GetBufferPointer(), blob1->GetBufferSize(),
        nullptr, &g_shaderPass1);
    blob1->Release();
    if (FAILED(hr))
    {
        g_lastError = "CreateComputeShader Pass1 failed";
        dc_cleanup();
        return false;
    }

    auto *blob2 = compile_hlsl(g_pass2Source, "main");
    if (!blob2)
    {
        dc_cleanup();
        return false;
    }
    hr = g_device->CreateComputeShader(
        blob2->GetBufferPointer(), blob2->GetBufferSize(),
        nullptr, &g_shaderPass2);
    blob2->Release();
    if (FAILED(hr))
    {
        g_lastError = "CreateComputeShader Pass2 failed";
        dc_cleanup();
        return false;
    }

    // 创建常量缓冲区
    D3D11_BUFFER_DESC cbDesc = {};
    cbDesc.ByteWidth = 128; // 足够大
    cbDesc.Usage = D3D11_USAGE_DYNAMIC;
    cbDesc.BindFlags = D3D11_BIND_CONSTANT_BUFFER;
    cbDesc.CPUAccessFlags = D3D11_CPU_ACCESS_WRITE;
    hr = g_device->CreateBuffer(&cbDesc, nullptr, &g_constantBuffer);
    if (FAILED(hr))
    {
        g_lastError = "CreateConstantBuffer failed";
        dc_cleanup();
        return false;
    }

    return true;
}

// ===== 按图像尺寸创建/重建资源 =====
static bool ensure_resources(int width, int height)
{
    if (g_device == nullptr)
        return false;

    // 如果尺寸没变, 直接复用
    if (g_curWidth == width && g_curHeight == height && g_inputTexture)
        return true;

    // 清理旧资源 (保留 device/context/shader)
    if (g_inputSRV)
    {
        g_inputSRV->Release();
        g_inputSRV = nullptr;
    }
    if (g_inputTexture)
    {
        g_inputTexture->Release();
        g_inputTexture = nullptr;
    }
    if (g_outputUAV)
    {
        g_outputUAV->Release();
        g_outputUAV = nullptr;
    }
    if (g_outputTexture)
    {
        g_outputTexture->Release();
        g_outputTexture = nullptr;
    }
    if (g_linearUAV)
    {
        g_linearUAV->Release();
        g_linearUAV = nullptr;
    }
    if (g_linearSRV)
    {
        g_linearSRV->Release();
        g_linearSRV = nullptr;
    }
    if (g_linearBuffer)
    {
        g_linearBuffer->Release();
        g_linearBuffer = nullptr;
    }
    if (g_lumUAV)
    {
        g_lumUAV->Release();
        g_lumUAV = nullptr;
    }
    if (g_lumBuffer)
    {
        g_lumBuffer->Release();
        g_lumBuffer = nullptr;
    }
    if (g_lumStaging)
    {
        g_lumStaging->Release();
        g_lumStaging = nullptr;
    }

    g_curWidth = width;
    g_curHeight = height;

    HRESULT hr;

    // 输入纹理 (RGBA 8-bit)
    D3D11_TEXTURE2D_DESC texDesc = {};
    texDesc.Width = width;
    texDesc.Height = height;
    texDesc.MipLevels = 1;
    texDesc.ArraySize = 1;
    texDesc.Format = DXGI_FORMAT_R8G8B8A8_UNORM;
    texDesc.SampleDesc.Count = 1;
    texDesc.Usage = D3D11_USAGE_DEFAULT;
    texDesc.BindFlags = D3D11_BIND_SHADER_RESOURCE | D3D11_BIND_UNORDERED_ACCESS;
    texDesc.CPUAccessFlags = 0;

    hr = g_device->CreateTexture2D(&texDesc, nullptr, &g_inputTexture);
    if (FAILED(hr))
    {
        g_lastError = "CreateInputTexture failed";
        return false;
    }

    D3D11_SHADER_RESOURCE_VIEW_DESC srvDesc = {};
    srvDesc.Format = DXGI_FORMAT_R8G8B8A8_UNORM;
    srvDesc.ViewDimension = D3D11_SRV_DIMENSION_TEXTURE2D;
    srvDesc.Texture2D.MipLevels = 1;
    hr = g_device->CreateShaderResourceView(g_inputTexture, &srvDesc, &g_inputSRV);
    if (FAILED(hr))
    {
        g_lastError = "CreateInputSRV failed";
        return false;
    }

    // 输出纹理 (RGBA 8-bit)
    texDesc.BindFlags = D3D11_BIND_UNORDERED_ACCESS;
    hr = g_device->CreateTexture2D(&texDesc, nullptr, &g_outputTexture);
    if (FAILED(hr))
    {
        g_lastError = "CreateOutputTexture failed";
        return false;
    }

    D3D11_UNORDERED_ACCESS_VIEW_DESC uavDesc = {};
    uavDesc.Format = DXGI_FORMAT_R8G8B8A8_UNORM;
    uavDesc.ViewDimension = D3D11_UAV_DIMENSION_TEXTURE2D;
    uavDesc.Texture2D.MipSlice = 0;
    hr = g_device->CreateUnorderedAccessView(g_outputTexture, &uavDesc, &g_outputUAV);
    if (FAILED(hr))
    {
        g_lastError = "CreateOutputUAV failed";
        return false;
    }

    // 线性缓冲区: width * height * float3
    int numPixels = width * height;
    g_linearBuffer = create_structured_buffer(sizeof(float) * 3, numPixels,
                                              nullptr, static_cast<D3D11_BIND_FLAG>(D3D11_BIND_UNORDERED_ACCESS | D3D11_BIND_SHADER_RESOURCE));
    if (!g_linearBuffer)
        return false;

    D3D11_UNORDERED_ACCESS_VIEW_DESC linUAVDesc = {};
    linUAVDesc.Format = DXGI_FORMAT_UNKNOWN;
    linUAVDesc.ViewDimension = D3D11_UAV_DIMENSION_BUFFER;
    linUAVDesc.Buffer.FirstElement = 0;
    linUAVDesc.Buffer.NumElements = numPixels;
    linUAVDesc.Buffer.Flags = 0;
    hr = g_device->CreateUnorderedAccessView(g_linearBuffer, &linUAVDesc, &g_linearUAV);
    if (FAILED(hr))
    {
        g_lastError = "CreateLinearUAV failed";
        return false;
    }

    // 线性缓冲区也作为 SRV 供 Pass2 读取
    D3D11_SHADER_RESOURCE_VIEW_DESC linSRVDesc = {};
    linSRVDesc.Format = DXGI_FORMAT_UNKNOWN;
    linSRVDesc.ViewDimension = D3D11_SRV_DIMENSION_BUFFER;
    linSRVDesc.Buffer.FirstElement = 0;
    linSRVDesc.Buffer.NumElements = numPixels;
    hr = g_device->CreateShaderResourceView(g_linearBuffer, &linSRVDesc, &g_linearSRV);
    if (FAILED(hr))
    {
        g_lastError = "CreateLinearSRV failed";
        return false;
    }

    // 亮度部分和: 每组一个 float
    g_maxGroupsX = (width + 15) / 16;
    g_maxGroupsY = (height + 15) / 16;
    int numGroups = g_maxGroupsX * g_maxGroupsY;

    g_lumBuffer = create_structured_buffer(sizeof(float), numGroups,
                                           nullptr, D3D11_BIND_UNORDERED_ACCESS);
    if (!g_lumBuffer)
        return false;

    D3D11_UNORDERED_ACCESS_VIEW_DESC lumUAVDesc = {};
    lumUAVDesc.Format = DXGI_FORMAT_UNKNOWN;
    lumUAVDesc.ViewDimension = D3D11_UAV_DIMENSION_BUFFER;
    lumUAVDesc.Buffer.FirstElement = 0;
    lumUAVDesc.Buffer.NumElements = numGroups;
    hr = g_device->CreateUnorderedAccessView(g_lumBuffer, &lumUAVDesc, &g_lumUAV);
    if (FAILED(hr))
    {
        g_lastError = "CreateLumUAV failed";
        return false;
    }

    // Staging buffer for luminance readback
    g_lumStaging = create_structured_buffer(sizeof(float), numGroups,
                                            nullptr, static_cast<D3D11_BIND_FLAG>(0), true);
    if (!g_lumStaging)
        return false;

    return true;
}

// ===== 处理 =====
bool dc_process(
    const uint8_t *input,
    int width,
    int height,
    uint8_t *output,
    float totalExposure,
    float gamma,
    float rAdj,
    float gAdj,
    float bAdj)
{
    if (!g_device || !g_context)
    {
        g_lastError = "DirectCompute not initialized";
        return false;
    }

    if (!ensure_resources(width, height))
        return false;

    HRESULT hr;

    // 1. 上传输入数据到纹理
    D3D11_BOX box = {0, 0, 0, (UINT)width, (UINT)height, 1};
    g_context->UpdateSubresource(g_inputTexture, 0, &box, input, width * 4, 0);

    // 2. 执行 Pass 1: sRGB → Linear + 亮度归约
    // 设置 Pass1 常量
    D3D11_MAPPED_SUBRESOURCE mapped;
    hr = g_context->Map(g_constantBuffer, 0, D3D11_MAP_WRITE_DISCARD, 0, &mapped);
    if (FAILED(hr))
    {
        g_lastError = "Map CB failed";
        return false;
    }

    struct CBPass1
    {
        unsigned int width, height, totalPixels, pad;
    };
    auto *cb1 = (CBPass1 *)mapped.pData;
    cb1->width = (unsigned int)width;
    cb1->height = (unsigned int)height;
    cb1->totalPixels = (unsigned int)(width * height);
    cb1->pad = 0;
    g_context->Unmap(g_constantBuffer, 0);

    // 绑定 Pass1 资源
    ID3D11UnorderedAccessView *pass1UAVs[] = {
        nullptr, // 输入纹理不通过 UAV 写, 通过 SRV 读... 但我们在 Pass1 里用的 RWTexture2D
        g_linearUAV,
        g_lumUAV};
    // 等等, RWTexture2D<float4> InputImage 需要 UAV, 而不能用 SRV
    // 需要创建 inputUAV
    // --- 重新创建 input 的 UAV ---
    ID3D11UnorderedAccessView *inputUAV = nullptr;
    D3D11_UNORDERED_ACCESS_VIEW_DESC inputUAVDesc = {};
    inputUAVDesc.Format = DXGI_FORMAT_R8G8B8A8_UNORM;
    inputUAVDesc.ViewDimension = D3D11_UAV_DIMENSION_TEXTURE2D;
    inputUAVDesc.Texture2D.MipSlice = 0;
    hr = g_device->CreateUnorderedAccessView(g_inputTexture, &inputUAVDesc, &inputUAV);
    if (FAILED(hr))
    {
        g_lastError = "CreateInputUAV failed";
        return false;
    }

    ID3D11UnorderedAccessView *pass1UAVs2[] = {inputUAV, g_linearUAV, g_lumUAV};
    g_context->CSSetUnorderedAccessViews(0, 3, pass1UAVs2, nullptr);
    g_context->CSSetConstantBuffers(0, 1, &g_constantBuffer);
    g_context->CSSetShader(g_shaderPass1, nullptr, 0);

    int groupsX = (width + 15) / 16;
    int groupsY = (height + 15) / 16;
    g_context->Dispatch(groupsX, groupsY, 1);

    // 解绑 Pass1 UAV
    ID3D11UnorderedAccessView *nullUAVs[] = {nullptr, nullptr, nullptr};
    g_context->CSSetUnorderedAccessViews(0, 3, nullUAVs, nullptr);

    inputUAV->Release();

    // 3. 读取亮度部分和 → CPU 计算平均亮度 → 自动伽马
    g_context->CopyResource(g_lumStaging, g_lumBuffer);

    D3D11_MAPPED_SUBRESOURCE lumMapped;
    hr = g_context->Map(g_lumStaging, 0, D3D11_MAP_READ, 0, &lumMapped);
    if (FAILED(hr))
    {
        g_lastError = "Map lum staging failed";
        return false;
    }

    int numGroups = groupsX * groupsY;
    float *lumData = (float *)lumMapped.pData;
    double totalLum = 0.0;
    for (int i = 0; i < numGroups; i++)
        totalLum += (double)lumData[i];
    g_context->Unmap(g_lumStaging, 0);

    double meanLum = totalLum / (double)(width * height);
    float autoGamma = 1.0f;
    bool useAutoGamma = (meanLum > 0.001 && meanLum < 0.999);
    if (useAutoGamma)
    {
        autoGamma = (float)(log(0.5) / log(meanLum));
        if (autoGamma < 0.3f)
            autoGamma = 0.3f;
        if (autoGamma > 3.0f)
            autoGamma = 3.0f;
    }

    // 4. 执行 Pass 2: 应用 HDR 处理 + 编码
    hr = g_context->Map(g_constantBuffer, 0, D3D11_MAP_WRITE_DISCARD, 0, &mapped);
    if (FAILED(hr))
    {
        g_lastError = "Map CB Pass2 failed";
        return false;
    }

    struct CBPass2
    {
        unsigned int width, height, totalPixels, pad;
        float totalExposure, gamma, rAdj, gAdj;
        float bAdj, autoGamma, useAutoGamma, pad2[2];
    };
    auto *cb2 = (CBPass2 *)mapped.pData;
    cb2->width = (unsigned int)width;
    cb2->height = (unsigned int)height;
    cb2->totalPixels = (unsigned int)(width * height);
    cb2->pad = 0;
    cb2->totalExposure = totalExposure;
    cb2->gamma = gamma;
    cb2->rAdj = rAdj;
    cb2->gAdj = gAdj;
    cb2->bAdj = bAdj;
    cb2->autoGamma = autoGamma;
    cb2->useAutoGamma = useAutoGamma ? 1.0f : 0.0f;
    g_context->Unmap(g_constantBuffer, 0);

    // 绑定 Pass2 资源: Linear SRV + Output UAV
    // Pass2 从 LinearInput SRV 读取, 写入 OutputImage UAV
    // 但我们的 Pass2 HLSL 用的是 RWStructuredBuffer<float3> LinearInput : register(u0)
    // 所以需要把 linearBuffer 绑定为 UAV
    ID3D11ShaderResourceView *pass2SRVs[] = {nullptr}; // not used
    ID3D11UnorderedAccessView *pass2UAVs[] = {g_linearUAV, g_outputUAV};
    g_context->CSSetUnorderedAccessViews(0, 2, pass2UAVs, nullptr);
    g_context->CSSetConstantBuffers(0, 1, &g_constantBuffer);
    g_context->CSSetShader(g_shaderPass2, nullptr, 0);

    g_context->Dispatch(groupsX, groupsY, 1);

    // 解绑
    ID3D11UnorderedAccessView *nullUAVs2[] = {nullptr, nullptr};
    g_context->CSSetUnorderedAccessViews(0, 2, nullUAVs2, nullptr);
    g_context->CSSetShader(nullptr, nullptr, 0);

    // 5. 回读输出数据
    D3D11_TEXTURE2D_DESC stagingDesc = {};
    stagingDesc.Width = width;
    stagingDesc.Height = height;
    stagingDesc.MipLevels = 1;
    stagingDesc.ArraySize = 1;
    stagingDesc.Format = DXGI_FORMAT_R8G8B8A8_UNORM;
    stagingDesc.SampleDesc.Count = 1;
    stagingDesc.Usage = D3D11_USAGE_STAGING;
    stagingDesc.CPUAccessFlags = D3D11_CPU_ACCESS_READ;
    stagingDesc.BindFlags = 0;

    ID3D11Texture2D *stagingTex = nullptr;
    hr = g_device->CreateTexture2D(&stagingDesc, nullptr, &stagingTex);
    if (FAILED(hr))
    {
        g_lastError = "CreateStagingTexture failed";
        return false;
    }

    g_context->CopyResource(stagingTex, g_outputTexture);

    D3D11_MAPPED_SUBRESOURCE outMapped;
    hr = g_context->Map(stagingTex, 0, D3D11_MAP_READ, 0, &outMapped);
    if (FAILED(hr))
    {
        stagingTex->Release();
        g_lastError = "Map output staging failed";
        return false;
    }

    const uint8_t *srcRow = (const uint8_t *)outMapped.pData;
    int srcStride = outMapped.RowPitch;
    for (int y = 0; y < height; y++)
    {
        memcpy(output + y * width * 4, srcRow + y * srcStride, width * 4);
    }

    g_context->Unmap(stagingTex, 0);
    stagingTex->Release();

    return true;
}

// ===== 清理 =====
void dc_cleanup()
{
    if (g_lumStaging)
    {
        g_lumStaging->Release();
        g_lumStaging = nullptr;
    }
    if (g_lumUAV)
    {
        g_lumUAV->Release();
        g_lumUAV = nullptr;
    }
    if (g_lumBuffer)
    {
        g_lumBuffer->Release();
        g_lumBuffer = nullptr;
    }
    if (g_linearSRV)
    {
        g_linearSRV->Release();
        g_linearSRV = nullptr;
    }
    if (g_linearUAV)
    {
        g_linearUAV->Release();
        g_linearUAV = nullptr;
    }
    if (g_linearBuffer)
    {
        g_linearBuffer->Release();
        g_linearBuffer = nullptr;
    }
    if (g_outputUAV)
    {
        g_outputUAV->Release();
        g_outputUAV = nullptr;
    }
    if (g_outputTexture)
    {
        g_outputTexture->Release();
        g_outputTexture = nullptr;
    }
    if (g_inputSRV)
    {
        g_inputSRV->Release();
        g_inputSRV = nullptr;
    }
    if (g_inputTexture)
    {
        g_inputTexture->Release();
        g_inputTexture = nullptr;
    }
    if (g_constantBuffer)
    {
        g_constantBuffer->Release();
        g_constantBuffer = nullptr;
    }
    if (g_shaderPass1)
    {
        g_shaderPass1->Release();
        g_shaderPass1 = nullptr;
    }
    if (g_shaderPass2)
    {
        g_shaderPass2->Release();
        g_shaderPass2 = nullptr;
    }
    if (g_context)
    {
        g_context->Release();
        g_context = nullptr;
    }
    if (g_device)
    {
        g_device->Release();
        g_device = nullptr;
    }

    g_curWidth = 0;
    g_curHeight = 0;
}

bool dc_is_available()
{
    return g_device != nullptr;
}

const char *dc_last_error()
{
    return g_lastError.c_str();
}
