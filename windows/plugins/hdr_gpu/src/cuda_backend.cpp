#include "cuda_backend.h"
#include <windows.h>
#include <cstdint>
#include <cstring>
#include <string>
#include <vector>
#include <cmath>

// ===== CUDA Driver API 函数指针 =====
// 动态加载 nvcuda.dll, 避免编译时链接依赖

// 类型定义
typedef int CUresult;
typedef void *CUmodule;
typedef void *CUfunction;
typedef void *CUdevice;
typedef void *CUcontext;
typedef void *CUstream;

#define CUDA_SUCCESS 0

// 函数指针
static HMODULE g_cudaLib = nullptr;

static CUresult (*cuInit_p)(unsigned int) = nullptr;
static CUresult (*cuDeviceGet_p)(CUdevice *, int) = nullptr;
static CUresult (*cuCtxCreate_p)(CUcontext *, unsigned int, CUdevice) = nullptr;
static CUresult (*cuCtxDestroy_p)(CUcontext) = nullptr;
static CUresult (*cuCtxSetCurrent_p)(CUcontext) = nullptr;
static CUresult (*cuModuleLoadData_p)(CUmodule *, const void *) = nullptr;
static CUresult (*cuModuleGetFunction_p)(CUfunction *, CUmodule, const char *) = nullptr;
static CUresult (*cuMemAlloc_p)(void **, size_t) = nullptr;
static CUresult (*cuMemFree_p)(void *) = nullptr;
static CUresult (*cuMemcpyHtoD_p)(void *, const void *, size_t) = nullptr;
static CUresult (*cuMemcpyDtoH_p)(void *, const void *, size_t) = nullptr;
static CUresult (*cuLaunchKernel_p)(CUfunction, unsigned int, unsigned int, unsigned int,
                                    unsigned int, unsigned int, unsigned int, unsigned int,
                                    void *, void **, void **) = nullptr;
static CUresult (*cuCtxSynchronize_p)(void) = nullptr;
static CUresult (*cuGetErrorString_p)(CUresult, const char **) = nullptr;
static CUresult (*cuModuleLoadDataEx_p)(CUmodule *, const void *, unsigned int, void *, void *) = nullptr;

// ===== 内部状态 =====
static CUdevice g_cudaDevice = nullptr;
static CUcontext g_cudaContext = nullptr;
static CUmodule g_cudaModule = nullptr;
static CUfunction g_kernelPass1 = nullptr;
static CUfunction g_kernelPass2 = nullptr;

static void *g_devInput = nullptr;
static void *g_devLinear = nullptr;
static void *g_devLum = nullptr;
static void *g_devOutput = nullptr;
static int g_curWidth = 0;
static int g_curHeight = 0;

static std::string g_lastError;

// ===== PTX 代码 (由 cuda_kernels.cu 编译生成) =====
// 如果 CUDA Toolkit 可用, 运行 nvcc -ptx cuda_kernels.cu -o cuda_kernels.ptx
// 然后将生成的 PTX 内容嵌入此处
//
// 由于 PTX 是文本格式, 我们可以直接将其作为字符串嵌入
static const char *g_ptxSource = nullptr;
static size_t g_ptxSize = 0;

void cuda_set_ptx(const char *ptx, size_t size)
{
    g_ptxSource = ptx;
    g_ptxSize = size;
}

// ===== 辅助 =====
static bool load_cuda_driver()
{
    if (g_cudaLib)
        return true;

    g_cudaLib = LoadLibraryW(L"nvcuda.dll");
    if (!g_cudaLib)
    {
        g_lastError = "nvcuda.dll not found";
        return false;
    }

// 尝试加载函数, 优先版本化名称 (CUDA 11+ 某些函数有 _v2 后缀)
#define LOAD_FUNC(var, name)                              \
    var = (decltype(var))GetProcAddress(g_cudaLib, name); \
    if (!var)                                             \
    {                                                     \
        g_lastError = "Failed to load " name;             \
        return false;                                     \
    }

#define LOAD_FUNC_V2(var, name_v2, name)                      \
    var = (decltype(var))GetProcAddress(g_cudaLib, name_v2);  \
    if (!var)                                                 \
    {                                                         \
        var = (decltype(var))GetProcAddress(g_cudaLib, name); \
        if (!var)                                             \
        {                                                     \
            g_lastError = "Failed to load " name;             \
            return false;                                     \
        }                                                     \
    }

    LOAD_FUNC(cuInit_p, "cuInit");
    LOAD_FUNC(cuDeviceGet_p, "cuDeviceGet");
    LOAD_FUNC_V2(cuCtxCreate_p, "cuCtxCreate_v2", "cuCtxCreate");
    LOAD_FUNC(cuCtxDestroy_p, "cuCtxDestroy");
    LOAD_FUNC_V2(cuCtxSetCurrent_p, "cuCtxSetCurrent_v2", "cuCtxSetCurrent");
    LOAD_FUNC(cuModuleLoadData_p, "cuModuleLoadData");
    LOAD_FUNC(cuModuleGetFunction_p, "cuModuleGetFunction");
    LOAD_FUNC_V2(cuMemAlloc_p, "cuMemAlloc_v2", "cuMemAlloc");
    LOAD_FUNC(cuMemFree_p, "cuMemFree");
    LOAD_FUNC_V2(cuMemcpyHtoD_p, "cuMemcpyHtoD_v2", "cuMemcpyHtoD");
    LOAD_FUNC_V2(cuMemcpyDtoH_p, "cuMemcpyDtoH_v2", "cuMemcpyDtoH");
    LOAD_FUNC(cuLaunchKernel_p, "cuLaunchKernel");
    LOAD_FUNC(cuCtxSynchronize_p, "cuCtxSynchronize");
    LOAD_FUNC(cuGetErrorString_p, "cuGetErrorString");

    // 可选
    cuModuleLoadDataEx_p = (decltype(cuModuleLoadDataEx_p))
        GetProcAddress(g_cudaLib, "cuModuleLoadDataEx");

    return true;
}

static const char *cuda_error_str(CUresult err)
{
    const char *str = nullptr;
    if (cuGetErrorString_p && cuGetErrorString_p(err, &str) == CUDA_SUCCESS && str)
        return str;
    return "Unknown CUDA error";
}

static bool check_cuda(CUresult err, const char *msg)
{
    if (err != CUDA_SUCCESS)
    {
        g_lastError = msg;
        g_lastError += ": ";
        g_lastError += cuda_error_str(err);
        return false;
    }
    return true;
}

// ===== PTX fallback: 编译时的 CUDA C++ 内核将被编译为 PTX =====
// 这里提供一个空的 PTX 占位, 实际使用时需要嵌入编译后的 PTX
// 如果 PTX 未设置, CUDA 初始化会失败, 自动回退到 DirectCompute

// ===== 初始化 =====
bool cuda_initialize()
{
    if (g_cudaContext)
        return true;

    g_lastError.clear();

    // 加载 CUDA 驱动
    if (!load_cuda_driver())
        return false;

    // 初始化 CUDA
    CUresult err = cuInit_p(0);
    if (!check_cuda(err, "cuInit failed"))
    {
        FreeLibrary(g_cudaLib);
        g_cudaLib = nullptr;
        return false;
    }

    // 获取设备 0
    err = cuDeviceGet_p(&g_cudaDevice, 0);
    if (!check_cuda(err, "cuDeviceGet failed"))
    {
        cuda_cleanup();
        return false;
    }

    // 创建上下文
    err = cuCtxCreate_p(&g_cudaContext, 0, g_cudaDevice);
    if (!check_cuda(err, "cuCtxCreate failed"))
    {
        cuda_cleanup();
        return false;
    }

    // 加载 PTX (如果有)
    if (g_ptxSource && g_ptxSize > 0)
    {
        CUmodule module = nullptr;
        if (cuModuleLoadDataEx_p)
        {
            err = cuModuleLoadDataEx_p(&module, g_ptxSource, 0, nullptr, nullptr);
        }
        else
        {
            err = cuModuleLoadData_p(&module, g_ptxSource);
        }

        if (!check_cuda(err, "cuModuleLoadData failed"))
        {
            // PTX 加载失败, 仍然可以继续 (DirectCompute 会接手)
            g_lastError += " (CUDA will be unavailable)";
            cuda_cleanup();
            return false;
        }

        g_cudaModule = module;

        // 获取内核函数
        err = cuModuleGetFunction_p(&g_kernelPass1, g_cudaModule, "pass1_srgb_to_linear");
        if (!check_cuda(err, "cuModuleGetFunction pass1 failed"))
        {
            cuda_cleanup();
            return false;
        }

        err = cuModuleGetFunction_p(&g_kernelPass2, g_cudaModule, "pass2_apply_hdr");
        if (!check_cuda(err, "cuModuleGetFunction pass2 failed"))
        {
            cuda_cleanup();
            return false;
        }
    }
    else
    {
        // 没有 PTX, CUDA 不可用
        g_lastError = "No PTX embedded, CUDA unavailable";
        cuda_cleanup();
        return false;
    }

    return true;
}

// ===== 确保设备缓冲区 =====
static bool ensure_device_buffers(int width, int height)
{
    if (g_curWidth == width && g_curHeight == height && g_devInput)
        return true;

    // 清理旧缓冲区
    if (g_devOutput)
    {
        cuMemFree_p(g_devOutput);
        g_devOutput = nullptr;
    }
    if (g_devLum)
    {
        cuMemFree_p(g_devLum);
        g_devLum = nullptr;
    }
    if (g_devLinear)
    {
        cuMemFree_p(g_devLinear);
        g_devLinear = nullptr;
    }
    if (g_devInput)
    {
        cuMemFree_p(g_devInput);
        g_devInput = nullptr;
    }

    g_curWidth = width;
    g_curHeight = height;

    int numPixels = width * height;
    int numGroups = ((width + 15) / 16) * ((height + 15) / 16);

    CUresult err;

    err = cuMemAlloc_p(&g_devInput, numPixels * 4); // uchar4
    if (!check_cuda(err, "cuMemAlloc input failed"))
        return false;

    err = cuMemAlloc_p(&g_devLinear, numPixels * sizeof(float) * 3); // float3
    if (!check_cuda(err, "cuMemAlloc linear failed"))
        return false;

    err = cuMemAlloc_p(&g_devLum, numGroups * sizeof(float));
    if (!check_cuda(err, "cuMemAlloc lum failed"))
        return false;

    err = cuMemAlloc_p(&g_devOutput, numPixels * 4); // uchar4
    if (!check_cuda(err, "cuMemAlloc output failed"))
        return false;

    return true;
}

// ===== 处理 =====
bool cuda_process(
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
    if (!g_cudaContext)
    {
        g_lastError = "CUDA not initialized";
        return false;
    }

    // 确保 context 对当前线程可见 (Worker Isolate 跨线程调用时需要)
    if (cuCtxSetCurrent_p)
    {
        CUresult ctxErr = cuCtxSetCurrent_p(g_cudaContext);
        if (ctxErr != CUDA_SUCCESS)
        {
            g_lastError = "cuCtxSetCurrent failed";
            return false;
        }
    }

    if (!ensure_device_buffers(width, height))
        return false;

    int numPixels = width * height;
    int groupsX = (width + 15) / 16;
    int groupsY = (height + 15) / 16;
    int numGroups = groupsX * groupsY;

    CUresult err;

    // 1. 上传输入数据
    err = cuMemcpyHtoD_p(g_devInput, input, numPixels * 4);
    if (!check_cuda(err, "cuMemcpyHtoD input failed"))
        return false;

    // 2. 执行 Pass 1 (pitch 未使用, 传 0)
    int pitchUnused = 0;
    void *pass1Args[] = {
        &g_devInput,
        &g_devLinear,
        &g_devLum,
        &width, &height, &pitchUnused
    };

    err = cuLaunchKernel_p(g_kernelPass1,
                           groupsX, groupsY, 1, // grid
                           16, 16, 1,           // block
                           0, nullptr,          // shared mem, stream
                           pass1Args, nullptr);
    if (!check_cuda(err, "cuLaunchKernel pass1 failed"))
        return false;

    err = cuCtxSynchronize_p();
    if (!check_cuda(err, "cuCtxSynchronize pass1 failed"))
        return false;

    // 3. 读取亮度部分和 → CPU 计算
    std::vector<float> hostLum(numGroups);
    err = cuMemcpyDtoH_p(hostLum.data(), g_devLum, numGroups * sizeof(float));
    if (!check_cuda(err, "cuMemcpyDtoH lum failed"))
        return false;

    double totalLum = 0.0;
    for (int i = 0; i < numGroups; i++)
        totalLum += (double)hostLum[i];

    double meanLum = totalLum / (double)numPixels;
    float autoGamma = 1.0f;
    bool useAutoGamma = (meanLum > 0.001 && meanLum < 0.999);
    if (useAutoGamma)
    {
        autoGamma = (float)(std::log(0.5) / std::log(meanLum));
        if (autoGamma < 0.3f)
            autoGamma = 0.3f;
        if (autoGamma > 3.0f)
            autoGamma = 3.0f;
    }

    // 4. 执行 Pass 2
    int useAutoGammaInt = useAutoGamma ? 1 : 0;
    void *pass2Args[] = {
        &g_devLinear,
        &g_devOutput,
        &width, &height,
        &totalExposure,
        &gamma,
        &rAdj, &gAdj, &bAdj,
        &autoGamma,
        &useAutoGammaInt};

    err = cuLaunchKernel_p(g_kernelPass2,
                           groupsX, groupsY, 1,
                           16, 16, 1,
                           0, nullptr,
                           pass2Args, nullptr);
    if (!check_cuda(err, "cuLaunchKernel pass2 failed"))
        return false;

    err = cuCtxSynchronize_p();
    if (!check_cuda(err, "cuCtxSynchronize pass2 failed"))
        return false;

    // 5. 回读输出
    err = cuMemcpyDtoH_p(output, g_devOutput, numPixels * 4);
    if (!check_cuda(err, "cuMemcpyDtoH output failed"))
        return false;

    return true;
}

// ===== 清理 =====
void cuda_cleanup()
{
    if (g_devOutput)
    {
        cuMemFree_p(g_devOutput);
        g_devOutput = nullptr;
    }
    if (g_devLum)
    {
        cuMemFree_p(g_devLum);
        g_devLum = nullptr;
    }
    if (g_devLinear)
    {
        cuMemFree_p(g_devLinear);
        g_devLinear = nullptr;
    }
    if (g_devInput)
    {
        cuMemFree_p(g_devInput);
        g_devInput = nullptr;
    }

    if (g_cudaModule)
    { /* cuModuleUnload not loaded */
        g_cudaModule = nullptr;
    }
    g_kernelPass1 = nullptr;
    g_kernelPass2 = nullptr;

    if (g_cudaContext)
    {
        cuCtxDestroy_p(g_cudaContext);
        g_cudaContext = nullptr;
    }
    g_cudaDevice = nullptr;

    if (g_cudaLib)
    {
        FreeLibrary(g_cudaLib);
        g_cudaLib = nullptr;
    }

// 重置函数指针
#define RESET_FUNC(name) name = nullptr
    RESET_FUNC(cuInit_p);
    RESET_FUNC(cuDeviceGet_p);
    RESET_FUNC(cuCtxCreate_p);
    RESET_FUNC(cuCtxSetCurrent_p);
    RESET_FUNC(cuCtxDestroy_p);
    RESET_FUNC(cuModuleLoadData_p);
    RESET_FUNC(cuModuleGetFunction_p);
    RESET_FUNC(cuMemAlloc_p);
    RESET_FUNC(cuMemFree_p);
    RESET_FUNC(cuMemcpyHtoD_p);
    RESET_FUNC(cuMemcpyDtoH_p);
    RESET_FUNC(cuLaunchKernel_p);
    RESET_FUNC(cuCtxSynchronize_p);
    RESET_FUNC(cuGetErrorString_p);
    RESET_FUNC(cuModuleLoadDataEx_p);

    g_curWidth = 0;
    g_curHeight = 0;
    g_lastError.clear();
}

bool cuda_is_available()
{
    return g_cudaContext != nullptr;
}

const char *cuda_last_error()
{
    return g_lastError.c_str();
}
