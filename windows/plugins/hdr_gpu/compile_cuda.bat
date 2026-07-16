@echo off
REM 编译 CUDA Kernels 为 PTX
REM 需要 CUDA Toolkit (nvcc.exe)

set NVCC=nvcc.exe
if exist "C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v12.x\bin\nvcc.exe" (
    set NVCC="C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v12.x\bin\nvcc.exe"
) else if exist "C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v11.8\bin\nvcc.exe" (
    set NVCC="C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v11.8\bin\nvcc.exe"
) else (
    echo nvcc.exe not found. CUDA backend will be unavailable.
    exit /b 1
)

echo Compiling cuda_kernels.cu to PTX...
%NVCC% -ptx src\cuda_kernels.cu -o shaders\cuda_kernels.ptx
if %ERRORLEVEL% neq 0 (
    echo Failed to compile CUDA kernels
    exit /b 1
)

echo CUDA kernels compiled successfully.
echo PTX saved to shaders\cuda_kernels.ptx
