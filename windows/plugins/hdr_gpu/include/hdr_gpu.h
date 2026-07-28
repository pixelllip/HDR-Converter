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

    // ========== HDR 预览 (D3D11 弹窗) ==========

    // 创建 HDR 预览窗口。
    // parent_hwnd: 父窗口句柄 (HWND), 传入 0 则自动查找 Flutter 窗口
    // width, height: 预览图像尺寸 (用于比例计算)
    // 返回 0 成功, 负值错误码。
    HDR_GPU_API int hdr_gpu_preview_create(void *parent_hwnd, int width, int height);

    // 显示 HDR 预览。
    // rgba: RGBA 8-bit 像素数据 (width * height * 4 bytes)
    // width, height: 图像尺寸
    // 自动做 sRGB→PQ 转换和缩放到窗口尺寸。
    // 返回 0 成功, 负值错误码。
    HDR_GPU_API int hdr_gpu_preview_show(const unsigned char *rgba, int width, int height);

    // 设置 HDR 预览窗口位置和尺寸 (相对于父窗口客户区)
    // 应在 show 之前调用
    HDR_GPU_API int hdr_gpu_preview_set_position(int x, int y, int width, int height);

    // 隐藏 HDR 预览窗口
    HDR_GPU_API void hdr_gpu_preview_hide();

    // 销毁 HDR 预览窗口及所有资源
    HDR_GPU_API void hdr_gpu_preview_destroy();

    // 检测当前预览窗口对应的显示器是否支持 HDR
    // 返回 1 支持, 0 不支持 (需先调用 preview_create)
    HDR_GPU_API int hdr_gpu_preview_is_hdr_available();

    // 轻量级系统 HDR 检测 (无需创建预览窗口)
    // 快速检测当前主显示器是否开启了 HDR
    // 返回 1 已开启, 0 未开启
    HDR_GPU_API int hdr_gpu_preview_check_system_hdr();

    // 预览窗口是否可见
    HDR_GPU_API int hdr_gpu_preview_is_visible();

    // 获取上次预览错误消息
    HDR_GPU_API const char *hdr_gpu_preview_error();

#ifdef __cplusplus
}
#endif

#endif // HDR_GPU_H
