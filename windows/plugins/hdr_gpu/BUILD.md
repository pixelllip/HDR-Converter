# hdr_gpu 插件构建指南

## 构建流程

```
1. compile_shaders.bat  → 编译 HLSL → CSO (运行时编译也可)
2. compile_cuda.bat     → 编译 CUDA → PTX (可选, 需要 CUDA Toolkit)
3. flutter build windows → CMake 自动构建 DLL
```

## 前提条件

### DirectCompute (必需)
- Windows 7+
- DirectX 11 运行时 (系统自带)
- 构建时需要 Windows SDK (Visual Studio 安装时可选)

### CUDA (可选)
- NVIDIA GPU
- CUDA Toolkit 11.x+ (从 NVIDIA 官网下载)
- 安装后确保 nvcc.exe 在 PATH 中

## 构建 DLL

```batch
cd windows\plugins\hdr_gpu

REM 编译 HLSL 着色器 (可选, 不编译则运行时内联编译)
cd shaders
compile_shaders.bat
cd ..

REM 编译 CUDA 内核 (可选, 需要 CUDA Toolkit)
compile_cuda.bat
cd ..\..\..

REM 构建整个 Flutter 项目
flutter build windows
```

生成的 `hdr_gpu.dll` 会自动输出到 `build/windows/runner/Release/` 目录。

## 调试

如需调试 GPU 后端, 可以在 Dart 侧检查日志:

```dart
final converter = HdrConverter.instance;
await converter.initialize();
print('GPU available: ${converter.isGpuAvailable}');
print('GPU backend: ${converter.gpuBackendName}');
```
