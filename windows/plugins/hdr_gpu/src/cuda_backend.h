#ifndef HDR_GPU_CUDA_BACKEND_H
#define HDR_GPU_CUDA_BACKEND_H

#include <cstdint>

// CUDA 后端 (通过动态加载 nvcuda.dll 实现)
// 编译时需要 CUDA Toolkit 生成 .ptx 文件
// 运行时通过 CUDA Driver API 动态加载

bool cuda_initialize();
void cuda_cleanup();
bool cuda_is_available();

// 完整管线
bool cuda_process(
    const uint8_t *input,
    int width,
    int height,
    uint8_t *output,
    float totalExposure,
    float gamma,
    float rAdj,
    float gAdj,
    float bAdj);

const char *cuda_last_error();

// 设置 PTX 代码 (由 CMake 构建时嵌入)
void cuda_set_ptx(const char *ptx, size_t size);

#endif
