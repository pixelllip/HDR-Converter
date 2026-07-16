#include <windows.h>
#include "../include/hdr_gpu.h"
#include "directcompute_backend.h"
#include "cuda_backend.h"
#include <cstring>
#include <string>
#include <mutex>

// ===== 全局状态 =====
static std::mutex g_mutex;
static HdrGpuBackend g_activeBackend = HDR_GPU_BACKEND_NONE;
static std::string g_lastError;
static bool g_initialized = false;

// ===== PTX 嵌入 (由 CMake 构建时生成) =====
#ifdef HAS_CUDA_PTX
#include "cuda_ptx_embed.h"
static const char *g_embeddedPtx = g_ptx_cuda_kernels;
static size_t g_embeddedPtxSize = g_ptx_cuda_kernels_size;
#else
static const char *g_embeddedPtx = nullptr;
static size_t g_embeddedPtxSize = 0;
#endif

// ===== 编译时嵌入的着色器 CSO (由构建脚本生成) =====
// compile_shaders.bat 编译 HLSL → CSO,
// 然后通过 xxd 或类似工具转换为 C 数组
// 如果 CSO 未嵌入, DirectCompute 后端会使用内联编译的 HLSL
static const void *g_embeddedPass1CSO = nullptr;
static size_t g_embeddedPass1Size = 0;
static const void *g_embeddedPass2CSO = nullptr;
static size_t g_embeddedPass2Size = 0;

// ===== DLL 入口 =====
BOOL APIENTRY DllMain(HMODULE hModule, DWORD reason, LPVOID lpReserved)
{
    switch (reason)
    {
    case DLL_PROCESS_ATTACH:
    case DLL_THREAD_ATTACH:
    case DLL_THREAD_DETACH:
    case DLL_PROCESS_DETACH:
        break;
    }
    return TRUE;
}

// ===== API 实现 =====

int hdr_gpu_init(int backend)
{
    std::lock_guard<std::mutex> lock(g_mutex);

    if (g_initialized)
        return HDR_GPU_SUCCESS;

    g_lastError.clear();

    // 如果 CSO 已嵌入, 传给 DirectCompute 后端
    if (g_embeddedPass1CSO && g_embeddedPass1Size > 0)
    {
        dc_set_shader_bytecode(
            g_embeddedPass1CSO, g_embeddedPass1Size,
            g_embeddedPass2CSO, g_embeddedPass2Size);
    }

    // 如果 PTX 已嵌入, 传给 CUDA 后端
    if (g_embeddedPtx && g_embeddedPtxSize > 0)
    {
        cuda_set_ptx(g_embeddedPtx, g_embeddedPtxSize);
    }

    bool cudaOk = false;
    bool dcOk = false;

    if (backend == HDR_GPU_BACKEND_CUDA || backend == HDR_GPU_BACKEND_NONE)
    {
        cudaOk = cuda_initialize();
        if (cudaOk)
        {
            g_activeBackend = HDR_GPU_BACKEND_CUDA;
            g_initialized = true;
            return HDR_GPU_SUCCESS;
        }
    }

    if (backend == HDR_GPU_BACKEND_DIRECTCOMPUTE || backend == HDR_GPU_BACKEND_NONE)
    {
        dcOk = dc_initialize();
        if (dcOk)
        {
            g_activeBackend = HDR_GPU_BACKEND_DIRECTCOMPUTE;
            g_initialized = true;
            return HDR_GPU_SUCCESS;
        }
    }

    // 全部失败
    g_lastError = "No GPU backend available. CUDA: ";
    g_lastError += cuda_last_error();
    g_lastError += " | DirectCompute: ";
    g_lastError += dc_last_error();

    // 记录可用但初始化失败的情况
    if (backend == HDR_GPU_BACKEND_CUDA && !cudaOk)
        g_lastError = std::string("CUDA init failed: ") + cuda_last_error();
    else if (backend == HDR_GPU_BACKEND_DIRECTCOMPUTE && !dcOk)
        g_lastError = std::string("DirectCompute init failed: ") + dc_last_error();

    g_activeBackend = HDR_GPU_BACKEND_NONE;
    g_initialized = false;
    return HDR_GPU_ERR_INIT;
}

int hdr_gpu_process(
    const unsigned char *input,
    int width,
    int height,
    unsigned char *output,
    float totalExposure,
    float gamma,
    float rAdj,
    float gAdj,
    float bAdj)
{
    if (!g_initialized || g_activeBackend == HDR_GPU_BACKEND_NONE)
    {
        g_lastError = "GPU not initialized. Call hdr_gpu_init first.";
        return HDR_GPU_ERR_INIT;
    }

    if (!input || !output || width <= 0 || height <= 0)
    {
        g_lastError = "Invalid parameters";
        return HDR_GPU_ERR_PARAM;
    }

    std::lock_guard<std::mutex> lock(g_mutex);

    bool ok = false;
    switch (g_activeBackend)
    {
    case HDR_GPU_BACKEND_CUDA:
        ok = cuda_process(input, width, height, output,
                          totalExposure, gamma, rAdj, gAdj, bAdj);
        if (!ok)
        {
            g_lastError = std::string("CUDA process error: ") + cuda_last_error();
            return HDR_GPU_ERR_PROCESS;
        }
        break;

    case HDR_GPU_BACKEND_DIRECTCOMPUTE:
        ok = dc_process(input, width, height, output,
                        totalExposure, gamma, rAdj, gAdj, bAdj);
        if (!ok)
        {
            g_lastError = std::string("DirectCompute process error: ") + dc_last_error();
            return HDR_GPU_ERR_PROCESS;
        }
        break;

    default:
        g_lastError = "No active GPU backend";
        return HDR_GPU_ERR_BACKEND;
    }

    return HDR_GPU_SUCCESS;
}

const char *hdr_gpu_error()
{
    return g_lastError.c_str();
}

void hdr_gpu_cleanup()
{
    std::lock_guard<std::mutex> lock(g_mutex);

    cuda_cleanup();
    dc_cleanup();

    g_activeBackend = HDR_GPU_BACKEND_NONE;
    g_initialized = false;
    g_lastError.clear();
}

int hdr_gpu_backend()
{
    return (int)g_activeBackend;
}
