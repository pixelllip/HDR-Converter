// =====================================================================
// hdr_preview.cpp — 应用内 HDR 预览窗口 (D3D11 + DXGI)
//
// 创建一个子弹出窗口作为 Flutter 窗口的 child，通过 DXGI 交换链
// 以 HDR10 (PQ ST.2084) 色彩空间渲染图像，实现真正的 HDR 预览。
//
// 需要: Windows 10+ (DXGI 1.6+), D3D11 运行时
// =====================================================================

#include <windows.h>
#include <d3d11.h>
#include <dxgi1_6.h>
#include <cmath>
#include <cstdio>
#include <cstring>
#include <string>
#include <mutex>



// ===== 内部状态 =====
static std::mutex g_previewMutex;
static HWND g_previewHwnd = nullptr;
static HWND g_parentHwnd = nullptr;
static ID3D11Device *g_previewDevice = nullptr;
static ID3D11DeviceContext *g_previewContext = nullptr;
static IDXGISwapChain1 *g_swapChain = nullptr;
static ID3D11RenderTargetView *g_rtv = nullptr;
static ID3D11Texture2D *g_stagingTexture = nullptr;

static int g_previewWidth = 0;
static int g_previewHeight = 0;
static bool g_hdrAvailable = false;
static bool g_previewVisible = false;
static std::string g_previewError;

// ===== 常量 =====
// PQ ST.2084 曲线参数
static const float PQ_M1 = 1305.0f / 8192.0f;    // 0.15930176
static const float PQ_M2 = 2523.0f / 4096.0f;    // 0.61596680
static const float PQ_C1 = 107.0f / 128.0f;      // 0.83593750
static const float PQ_C2 = 2413.0f / 128.0f;     // 18.85156250
static const float PQ_C3 = 2392.0f / 128.0f;     // 18.68750000

// HDR 峰值亮度 (nits) — 用于将 SDR 值映射到 HDR 范围
static const float HDR_PEAK_NITS = 1000.0f;

// ===== Window 类名 =====
static const wchar_t kPreviewWindowClass[] = L"HDR_PREVIEW_WINDOW";
static const wchar_t kFlutterWindowClass[] = L"FLUTTER_RUNNER_WIN32_WINDOW";

// ===== Helper: sRGB → Linear =====
static float sRGBToLinear(float c)
{
    if (c <= 0.04045f)
        return c / 12.92f;
    return powf((c + 0.055f) / 1.055f, 2.4f);
}

// ===== Helper: Linear → PQ (ST.2084) =====
static float linearToPQ(float linearNits)
{
    // PQ 编码: 输入 nits, 输出 [0,1]
    float L = linearNits / 10000.0f; // 归一化到 10000 nits
    if (L < 1e-6f) return 0.0f;
    float N = powf(L, PQ_M1);
    float p = powf((PQ_C1 + PQ_C2 * N) / (1.0f + PQ_C3 * N), PQ_M2);
    return p;
}

// ===== 窗口过程 =====
static LRESULT CALLBACK PreviewWndProc(HWND hwnd, UINT msg, WPARAM wParam, LPARAM lParam)
{
    switch (msg)
    {
    case WM_ERASEBKGND:
        return 1; // 阻止背景擦除（由 D3D 绘制）
    case WM_CLOSE:
        ShowWindow(hwnd, SW_HIDE);
        return 0;
    case WM_SIZE:
        // 调整大小时重建交换链
        if (g_swapChain && wParam != SIZE_MINIMIZED)
        {
            int newWidth = LOWORD(lParam);
            int newHeight = HIWORD(lParam);
            if (newWidth > 0 && newHeight > 0)
            {
                g_previewContext->OMSetRenderTargets(0, nullptr, nullptr);
                if (g_rtv) { g_rtv->Release(); g_rtv = nullptr; }

                IDXGISwapChain2 *sc2 = nullptr;
                if (SUCCEEDED(g_swapChain->QueryInterface(&sc2)))
                {
                    sc2->ResizeBuffers(0, newWidth, newHeight, DXGI_FORMAT_UNKNOWN, 0);
                    sc2->Release();
                }

                // 重建 RTV
                ID3D11Texture2D *backBuffer = nullptr;
                if (SUCCEEDED(g_swapChain->GetBuffer(0, __uuidof(ID3D11Texture2D), (void**)&backBuffer)))
                {
                    g_previewDevice->CreateRenderTargetView(backBuffer, nullptr, &g_rtv);
                    backBuffer->Release();
                }
                g_previewWidth = newWidth;
                g_previewHeight = newHeight;
            }
        }
        return 0;
    }
    return DefWindowProc(hwnd, msg, wParam, lParam);
}

// ===== 注册窗口类 =====
static bool RegisterPreviewWindowClass()
{
    static bool registered = false;
    if (registered) return true;

    WNDCLASS wc = {};
    wc.lpfnWndProc = PreviewWndProc;
    wc.hInstance = GetModuleHandle(nullptr);
    wc.hCursor = LoadCursor(nullptr, IDC_ARROW);
    wc.lpszClassName = kPreviewWindowClass;
    wc.hbrBackground = nullptr; // 无背景刷
    wc.style = CS_HREDRAW | CS_VREDRAW;

    registered = (RegisterClass(&wc) != 0);
    return registered;
}

// ===== 检测指定窗口所在显示器的 HDR 状态 =====
// 通过枚举 DXGI 输出匹配 HMONITOR 来确定目标显示器
static int _GetMonitorHdr(HMONITOR hMonitor)
{
    if (!hMonitor) return 0;

    IDXGIFactory1 *factory = nullptr;
    if (FAILED(CreateDXGIFactory1(__uuidof(IDXGIFactory1), (void**)&factory)))
        return 0;

    int hdr = 0;
    IDXGIAdapter1 *adapter = nullptr;
    for (UINT ai = 0; factory->EnumAdapters1(ai, &adapter) != DXGI_ERROR_NOT_FOUND; ai++)
    {
        IDXGIOutput *output = nullptr;
        for (UINT oi = 0; adapter->EnumOutputs(oi, &output) != DXGI_ERROR_NOT_FOUND; oi++)
        {
            DXGI_OUTPUT_DESC outputDesc;
            if (SUCCEEDED(output->GetDesc(&outputDesc)) && outputDesc.Monitor == hMonitor)
            {
                IDXGIOutput6 *output6 = nullptr;
                if (SUCCEEDED(output->QueryInterface(&output6)))
                {
                    DXGI_OUTPUT_DESC1 desc1;
                    if (SUCCEEDED(output6->GetDesc1(&desc1)))
                    {
                        hdr = (desc1.ColorSpace == DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020 ||
                               desc1.MaxLuminance > 200.0f) ? 1 : 0;
                    }
                    output6->Release();
                }
                output->Release();
                adapter->Release();
                factory->Release();
                return hdr;
            }
            output->Release();
            output = nullptr;
        }
        adapter->Release();
        adapter = nullptr;
    }
    factory->Release();
    return hdr;
}

// ===== 获取 Flutter 窗口所在显示器的 HDR 状态 =====
static int _GetParentWindowHdr()
{
    HWND parent = g_parentHwnd;
    if (!parent)
    {
        parent = FindWindowW(kFlutterWindowClass, nullptr);
        if (!parent) parent = GetForegroundWindow();
    }
    HMONITOR hMon = MonitorFromWindow(parent, MONITOR_DEFAULTTONEAREST);
    return _GetMonitorHdr(hMon);
}

// ===== 获取可用的 HDR 刷新率 =====
static DXGI_RATIONAL GetBestRefreshRate(IDXGIAdapter1 *adapter, int width, int height)
{
    IDXGIOutput *output = nullptr;
    DXGI_RATIONAL best = {60, 1};
    if (FAILED(adapter->EnumOutputs(0, &output)))
        return best;

    UINT numModes = 0;
    DXGI_MODE_DESC *displayModes = nullptr;
    if (SUCCEEDED(output->GetDisplayModeList(DXGI_FORMAT_R10G10B10A2_UNORM, 0, &numModes, nullptr)) && numModes > 0)
    {
        displayModes = new DXGI_MODE_DESC[numModes];
        if (SUCCEEDED(output->GetDisplayModeList(DXGI_FORMAT_R10G10B10A2_UNORM, 0, &numModes, displayModes)))
        {
            for (UINT i = 0; i < numModes; i++)
            {
                if (displayModes[i].Width == (UINT)width && displayModes[i].Height == (UINT)height)
                {
                    if (displayModes[i].RefreshRate.Numerator > best.Numerator)
                        best = displayModes[i].RefreshRate;
                }
            }
        }
        delete[] displayModes;
    }
    output->Release();
    return best;
}

// ===== API 实现 =====

int hdr_gpu_preview_create(void *parent_hwnd, int width, int height)
{
    std::lock_guard<std::mutex> lock(g_previewMutex);

    if (g_previewHwnd)
        return 0; // 已创建

    g_previewError.clear();

    if (!RegisterPreviewWindowClass())
    {
        g_previewError = "Failed to register preview window class";
        return -1;
    }

    // 如果未传入 parent_hwnd，尝试自动查找 Flutter 窗口
    if (parent_hwnd == nullptr)
    {
        g_parentHwnd = FindWindowW(kFlutterWindowClass, nullptr);
        if (!g_parentHwnd)
        {
            g_parentHwnd = GetForegroundWindow();
        }
    }
    else
    {
        g_parentHwnd = (HWND)parent_hwnd;
    }

    if (!g_parentHwnd)
    {
        g_previewError = "Cannot find parent window";
        return -1;
    }

    g_previewWidth = width;
    g_previewHeight = height;

    // 创建预览窗口 (初始隐藏, 稍后通过 set_position 定位)
    g_previewHwnd = CreateWindowExW(
        WS_EX_NOACTIVATE | WS_EX_LAYERED | WS_EX_TRANSPARENT,
        kPreviewWindowClass,
        L"HDR Preview",
        WS_POPUP,
        0, 0, width, height,
        g_parentHwnd,
        nullptr,
        GetModuleHandle(nullptr),
        nullptr);

    if (!g_previewHwnd)
    {
        g_previewError = "Failed to create preview window";
        return -1;
    }

    SetLayeredWindowAttributes(g_previewHwnd, 0, 255, LWA_ALPHA);

    // 创建 D3D11 设备
    UINT createFlags = D3D11_CREATE_DEVICE_BGRA_SUPPORT;
#ifdef _DEBUG
    createFlags |= D3D11_CREATE_DEVICE_DEBUG;
#endif

    D3D_FEATURE_LEVEL featureLevels[] = { D3D_FEATURE_LEVEL_11_0 };
    D3D_FEATURE_LEVEL outFeatureLevel;

    HRESULT hr = D3D11CreateDevice(
        nullptr, D3D_DRIVER_TYPE_HARDWARE, nullptr, createFlags,
        featureLevels, 1, D3D11_SDK_VERSION,
        &g_previewDevice, &outFeatureLevel, &g_previewContext);

    if (FAILED(hr))
    {
        g_previewError = "Failed to create D3D11 device for preview";
        DestroyWindow(g_previewHwnd);
        g_previewHwnd = nullptr;
        return -1;
    }

    // 用 RAII lambda 管理交换链创建，避免 goto 跳过初始化
    auto cleanupAndFail = [&](const char *msg) -> int
    {
        g_previewError = msg;
        if (g_previewDevice) { g_previewDevice->Release(); g_previewDevice = nullptr; }
        if (g_previewContext) { g_previewContext->Release(); g_previewContext = nullptr; }
        if (g_previewHwnd) { DestroyWindow(g_previewHwnd); g_previewHwnd = nullptr; }
        g_parentHwnd = nullptr;
        g_swapChain = nullptr;
        g_rtv = nullptr;
        g_stagingTexture = nullptr;
        return -1;
    };

    // 创建 DXGI 交换链 (在独立作用域内声明局部变量)
    {
        IDXGIDevice *dxgiDevice = nullptr;
        IDXGIAdapter1 *adapter = nullptr;
        IDXGIFactory2 *factory = nullptr;

        hr = g_previewDevice->QueryInterface(&dxgiDevice);
        if (FAILED(hr)) return cleanupAndFail("Failed to get DXGI device");

        hr = dxgiDevice->GetAdapter((IDXGIAdapter**)&adapter);
        if (FAILED(hr)) { dxgiDevice->Release(); return cleanupAndFail("Failed to get adapter"); }

        hr = adapter->GetParent(__uuidof(IDXGIFactory2), (void**)&factory);
        if (FAILED(hr)) { adapter->Release(); dxgiDevice->Release(); return cleanupAndFail("Failed to get factory"); }

        // 检测窗口所在显示器的 HDR 状态
        g_hdrAvailable = _GetParentWindowHdr() != 0;
        {
            char buf[128];
            sprintf_s(buf, "[HDR_GPU] create: parent HDR=%d\n", g_hdrAvailable);
            OutputDebugStringA(buf);
        }

        // 交换链描述
        DXGI_SWAP_CHAIN_DESC1 scDesc = {};
        scDesc.Width = width;
        scDesc.Height = height;
        scDesc.Format = DXGI_FORMAT_R10G10B10A2_UNORM;
        scDesc.Stereo = FALSE;
        scDesc.SampleDesc.Count = 1;
        scDesc.SampleDesc.Quality = 0;
        scDesc.BufferUsage = DXGI_USAGE_RENDER_TARGET_OUTPUT;
        scDesc.BufferCount = 2;
        scDesc.Scaling = DXGI_SCALING_STRETCH;
        scDesc.SwapEffect = DXGI_SWAP_EFFECT_FLIP_DISCARD;
        scDesc.AlphaMode = DXGI_ALPHA_MODE_IGNORE;
        scDesc.Flags = DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING;

        hr = factory->CreateSwapChainForHwnd(
            g_previewDevice, g_previewHwnd, &scDesc, nullptr, nullptr, &g_swapChain);

        if (FAILED(hr))
        {
            factory->Release(); adapter->Release(); dxgiDevice->Release();
            return cleanupAndFail("Failed to create swap chain");
        }

        // 防止 ALT+ENTER 全屏
        factory->MakeWindowAssociation(g_previewHwnd, DXGI_MWA_NO_ALT_ENTER);

        factory->Release();
        adapter->Release();
        dxgiDevice->Release();
    }

    // 获取 back buffer 并创建 RTV
    ID3D11Texture2D *backBuffer = nullptr;
    hr = g_swapChain->GetBuffer(0, __uuidof(ID3D11Texture2D), (void**)&backBuffer);
    if (SUCCEEDED(hr))
    {
        g_previewDevice->CreateRenderTargetView(backBuffer, nullptr, &g_rtv);
        backBuffer->Release();
    }

    // 设置 HDR 色彩空间 (如果显示器支持)
    if (g_hdrAvailable)
    {
        IDXGISwapChain3 *sc3 = nullptr;
        if (SUCCEEDED(g_swapChain->QueryInterface(&sc3)))
        {
            DXGI_COLOR_SPACE_TYPE colorSpace = DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020;
            sc3->SetColorSpace1(colorSpace);
            sc3->Release();
        }
    }

    // 创建 staging texture 用于上传像素
    D3D11_TEXTURE2D_DESC stagingDesc = {};
    stagingDesc.Width = width;
    stagingDesc.Height = height;
    stagingDesc.MipLevels = 1;
    stagingDesc.ArraySize = 1;
    stagingDesc.Format = DXGI_FORMAT_R10G10B10A2_UNORM;
    stagingDesc.SampleDesc.Count = 1;
    stagingDesc.SampleDesc.Quality = 0;
    stagingDesc.Usage = D3D11_USAGE_DYNAMIC;
    stagingDesc.BindFlags = D3D11_BIND_SHADER_RESOURCE;
    stagingDesc.CPUAccessFlags = D3D11_CPU_ACCESS_WRITE;
    stagingDesc.MiscFlags = 0;

    hr = g_previewDevice->CreateTexture2D(&stagingDesc, nullptr, &g_stagingTexture);
    if (FAILED(hr))
    {
        g_previewError = "Failed to create staging texture";
        return cleanupAndFail("Failed to create staging texture");
    }

    ShowWindow(g_previewHwnd, SW_HIDE);
    return 0;
}

int hdr_gpu_preview_show(const unsigned char *rgba, int width, int height)
{
    std::lock_guard<std::mutex> lock(g_previewMutex);

    if (!g_previewDevice || !g_previewContext || !g_swapChain || !g_previewHwnd)
    {
        g_previewError = "Preview not initialized";
        return -1;
    }

    // 使窗口可见 (位置由 set_position 预先设置)
    ShowWindow(g_previewHwnd, SW_SHOWNA);

    // 每次显示时检查当前显示器 HDR 状态 (支持跨显示器拖拽)
    {
        int currentHdr = _GetParentWindowHdr();
        if (currentHdr != (g_hdrAvailable ? 1 : 0))
        {
            g_hdrAvailable = currentHdr != 0;
            char buf[128];
            sprintf_s(buf, "[HDR_GPU] monitor changed, HDR=%d\n", g_hdrAvailable);
            OutputDebugStringA(buf);

            // 更新交换链色彩空间
            IDXGISwapChain3 *sc3 = nullptr;
            if (SUCCEEDED(g_swapChain->QueryInterface(&sc3)))
            {
                DXGI_COLOR_SPACE_TYPE cs = g_hdrAvailable
                    ? DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020
                    : DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709;
                sc3->SetColorSpace1(cs);
                sc3->Release();
            }
        }
    }

    // 将 RGBA 8-bit 数据转换为 R10G10B10A2 PQ 编码
    // 并写入 staging texture
    D3D11_MAPPED_SUBRESOURCE mapped;
    HRESULT hr = g_previewContext->Map(g_stagingTexture, 0, D3D11_MAP_WRITE_DISCARD, 0, &mapped);
    if (FAILED(hr))
    {
        g_previewError = "Failed to map staging texture";
        return -1;
    }

    // 对输入图像进行双线性缩放到预览窗口尺寸
    uint32_t *dstRow = (uint32_t *)mapped.pData;
    int dstPitch = mapped.RowPitch / 4; // R10G10B10A2 像素数/行

    float scaleX = (float)width / g_previewWidth;
    float scaleY = (float)height / g_previewHeight;

    for (int y = 0; y < g_previewHeight; y++)
    {
        for (int x = 0; x < g_previewWidth; x++)
        {
            float srcX = x * scaleX;
            float srcY = y * scaleY;

            // 最近邻采样 (性能优先)
            int sx = (int)(srcX + 0.5f);
            int sy = (int)(srcY + 0.5f);
            if (sx >= width) sx = width - 1;
            if (sy >= height) sy = height - 1;

            const unsigned char *srcPixel = rgba + (sy * width + sx) * 4;
            float r = srcPixel[0] / 255.0f;
            float g = srcPixel[1] / 255.0f;
            float b = srcPixel[2] / 255.0f;
            float a = srcPixel[3] / 255.0f;

            // sRGB → Linear
            float lr = sRGBToLinear(r);
            float lg = sRGBToLinear(g);
            float lb = sRGBToLinear(b);

            // 映射到 HDR 亮度范围
            float hdrR = lr * HDR_PEAK_NITS;
            float hdrG = lg * HDR_PEAK_NITS;
            float hdrB = lb * HDR_PEAK_NITS;

            // PQ 编码
            float pqR = linearToPQ(hdrR);
            float pqG = linearToPQ(hdrG);
            float pqB = linearToPQ(hdrB);
            float pqA = 1.0f; // 不透明

            // 打包为 R10G10B10A2
            uint32_t pixel =
                ((uint32_t)(pqR * 1023.0f + 0.5f) & 0x3FF) |
                (((uint32_t)(pqG * 1023.0f + 0.5f) & 0x3FF) << 10) |
                (((uint32_t)(pqB * 1023.0f + 0.5f) & 0x3FF) << 20) |
                (((uint32_t)(pqA * 3.0f + 0.5f) & 0x3) << 30);

            dstRow[y * dstPitch + x] = pixel;
        }
    }

    g_previewContext->Unmap(g_stagingTexture, 0);

    // 将 staging texture 拷贝到 back buffer
    ID3D11Texture2D *backBuffer = nullptr;
    hr = g_swapChain->GetBuffer(0, __uuidof(ID3D11Texture2D), (void**)&backBuffer);
    if (FAILED(hr))
    {
        g_previewError = "Failed to get back buffer";
        return -1;
    }

    g_previewContext->CopyResource(backBuffer, g_stagingTexture);
    backBuffer->Release();

    // 呈现 — 使用 AllowTearing 时传 0 以允许垂直同步
    DXGI_PRESENT_PARAMETERS presentParams = {};
    g_swapChain->Present1(1, 0, &presentParams);

    g_previewVisible = true;
    return 0;
}

int hdr_gpu_preview_set_position(int x, int y, int width, int height)
{
    std::lock_guard<std::mutex> lock(g_previewMutex);
    if (!g_previewHwnd)
        return -1;

    g_previewWidth = width;
    g_previewHeight = height;

    SetWindowPos(g_previewHwnd, HWND_TOP, x, y, width, height,
                 SWP_NOACTIVATE | SWP_HIDEWINDOW);
    return 0;
}

void hdr_gpu_preview_hide()
{
    std::lock_guard<std::mutex> lock(g_previewMutex);
    if (g_previewHwnd)
    {
        ShowWindow(g_previewHwnd, SW_HIDE);
    }
    g_previewVisible = false;
}

void hdr_gpu_preview_destroy()
{
    std::lock_guard<std::mutex> lock(g_previewMutex);

    if (g_previewContext)
        g_previewContext->ClearState();

    if (g_rtv) { g_rtv->Release(); g_rtv = nullptr; }
    if (g_swapChain) { g_swapChain->Release(); g_swapChain = nullptr; }
    if (g_stagingTexture) { g_stagingTexture->Release(); g_stagingTexture = nullptr; }
    if (g_previewContext) { g_previewContext->Release(); g_previewContext = nullptr; }
    if (g_previewDevice) { g_previewDevice->Release(); g_previewDevice = nullptr; }

    if (g_previewHwnd)
    {
        DestroyWindow(g_previewHwnd);
        g_previewHwnd = nullptr;
    }

    g_parentHwnd = nullptr;
    g_previewVisible = false;
    g_hdrAvailable = false;
    g_previewWidth = 0;
    g_previewHeight = 0;
}

int hdr_gpu_preview_is_hdr_available()
{
    return g_hdrAvailable ? 1 : 0;
}

// 轻量系统 HDR 检测 — 查询 Flutter 窗口所在显示器
int hdr_gpu_preview_check_system_hdr()
{
    int hdr = _GetParentWindowHdr();
    char buf[128];
    sprintf_s(buf, "[HDR_GPU] check_system_hdr: %d\n", hdr);
    OutputDebugStringA(buf);
    return hdr;
}

int hdr_gpu_preview_is_visible()
{
    return g_previewVisible ? 1 : 0;
}

const char *hdr_gpu_preview_error()
{
    return g_previewError.c_str();
}
