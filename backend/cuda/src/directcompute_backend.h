#ifndef HDR_GPU_DIRECTCOMPUTE_H
#define HDR_GPU_DIRECTCOMPUTE_H

#include <cstdint>

// DirectCompute 后端
// 由主模块调用，内部封装 DirectX 11 + Compute Shader

bool dc_initialize();
void dc_cleanup();
bool dc_is_available();

// 完整管线
bool dc_process(
    const uint8_t *input,
    int width,
    int height,
    uint8_t *output,
    float totalExposure,
    float gamma,
    float rAdj,
    float gAdj,
    float bAdj);

const char *dc_last_error();

// 设置预编译的着色器字节码 (可选, 不设置则运行时内联编译 HLSL)
void dc_set_shader_bytecode(const void *pass1, size_t pass1Size,
                            const void *pass2, size_t pass2Size);

#endif
