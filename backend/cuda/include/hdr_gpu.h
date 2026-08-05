#ifndef HDR_GPU_H
#define HDR_GPU_H

#ifdef HDR_GPU_EXPORTS
#define HDR_GPU_API __declspec(dllexport)
#else
#define HDR_GPU_API __declspec(dllimport)
#endif

#define HDR_GPU_SUCCESS 0
#define HDR_GPU_ERR_INIT -1
#define HDR_GPU_ERR_BACKEND -2
#define HDR_GPU_ERR_PROCESS -3
#define HDR_GPU_ERR_MEMORY -4
#define HDR_GPU_ERR_PARAM -5

// GPU 后端类型
typedef enum
{
    HDR_GPU_BACKEND_NONE = 0,
    HDR_GPU_BACKEND_CUDA = 1,
    HDR_GPU_BACKEND_DIRECTCOMPUTE = 2
} HdrGpuBackend;

#ifdef __cplusplus
extern "C"
{
#endif

    // 初始化 GPU 后端。
    // backend: HDR_GPU_BACKEND_CUDA, _DIRECTCOMPUTE, 或 _NONE (自动检测, 优先 CUDA)
    // 返回 0 成功, 负值错误码。
    HDR_GPU_API int hdr_gpu_init(int backend);

    // 处理图像 (完整管线)。
    // input:    RGBA 8-bit 输入 (width*height*4 bytes)
    // output:   RGBA 8-bit 输出 (width*height*4 bytes, 由调用者分配)
    // width, height: 图像尺寸
    // totalExposure: settings.totalExposure - 1
    // gamma:         settings.gamma
    // rAdj, gAdj, bAdj: RGB 通道调整
    // 返回 0 成功, 负值错误码。
    HDR_GPU_API int hdr_gpu_process(
        const unsigned char *input,
        int width,
        int height,
        unsigned char *output,
        float totalExposure,
        float gamma,
        float rAdj,
        float gAdj,
        float bAdj);

    // 获取上次错误消息
    HDR_GPU_API const char *hdr_gpu_error();

    // 释放所有 GPU 资源
    HDR_GPU_API void hdr_gpu_cleanup();

    // 获取当前后端类型
    HDR_GPU_API int hdr_gpu_backend();

#ifdef __cplusplus
}
#endif

#endif // HDR_GPU_H
