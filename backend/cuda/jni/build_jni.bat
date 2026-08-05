@echo off
rem ============================================================================
rem 构建 hdr_gpu_jni.dll（CUDA 加速 JNI 桥，供 Kotlin 后端 System.load 调用）
rem
rem 前置条件：
rem   1. CUDA Toolkit 12+（nvcc 在 PATH）
rem   2. JDK（含 jni.h）——自动探测；可用 JAVA_HOME 覆盖
rem
rem 输出：..\hdr_gpu_jni.dll（即 backend/cuda/hdr_gpu_jni.dll）
rem ============================================================================
setlocal

rem ---- 自动探测 JDK（含 include\jni.h）----
set "JDK="
if not "%JAVA_HOME%"=="" if exist "%JAVA_HOME%\include\jni.h" set "JDK=%JAVA_HOME%"
if "%JDK%"=="" if exist "C:\Users\Administrator\.gradle\jdks\jetbrains_s_r_o_-21-amd64-windows.2\include\jni.h" set "JDK=C:\Users\Administrator\.gradle\jdks\jetbrains_s_r_o_-21-amd64-windows.2"
if "%JDK%"=="" if exist "C:\Program Files\Java\jdk-25\include\jni.h" set "JDK=C:\Program Files\Java\jdk-25"
if "%JDK%"=="" (
    echo [ERROR] 未找到含 jni.h 的 JDK，请设置 JAVA_HOME 指向完整 JDK
    exit /b 1
)

where nvcc >nul 2>nul
if errorlevel 1 (
    echo [ERROR] 未找到 nvcc，请安装 CUDA Toolkit 并加入 PATH
    exit /b 1
)

set SRC=%~dp0hdr_gpu_jni.cu
set OUT=%~dp0..\hdr_gpu_jni.dll

echo [INFO] JDK=%JDK%

rem ---- 自动探测 Visual Studio（vcvarsall.bat）----
set "VCVARS="
if exist "D:\Program Files\Visual Studio\VC\Auxiliary\Build\vcvarsall.bat" set "VCVARS=D:\Program Files\Visual Studio\VC\Auxiliary\Build\vcvarsall.bat"
if "%VCVARS%"=="" if exist "C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvars64.bat" set "VCVARS=C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvars64.bat"
if "%VCVARS%"=="" if exist "C:\Program Files\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat" set "VCVARS=C:\Program Files\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat"
if "%VCVARS%"=="" (
    echo [ERROR] 未找到 Visual Studio vcvars，请修改本脚本指定 VCVARS 路径
    exit /b 1
)

where nvcc >nul 2>nul
if errorlevel 1 (
    echo [ERROR] 未找到 nvcc，请安装 CUDA Toolkit 并加入 PATH
    exit /b 1
)

echo [INFO] vcvars=%VCVARS%
echo [INFO] nvcc --version:
nvcc --version | findstr /b "Cuda compilation tools"

rem 初始化 MSVC x64 环境（nvcc 需要 cl.exe）
call "%VCVARS%" x64 >nul
if errorlevel 1 (
    echo [ERROR] vcvarsall 初始化失败
    exit /b 1
)

echo [BUILD] nvcc -shared ...
nvcc -shared -O3 -gencode arch=compute_75,code=compute_75 -cudart=static ^
    -I"%JDK%\include" -I"%JDK%\include\win32" ^
    -Xcompiler "/MD /wd4819" "%SRC%" -o "%OUT%"

if errorlevel 1 (
    echo [ERROR] 构建失败
    exit /b 1
)

echo [OK] 已生成 %OUT%
endlocal
