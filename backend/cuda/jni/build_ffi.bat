@echo off
rem Build hdr_gpu_ffi.dll (C ABI export for Rust hdrconv --features gpu FFI).
rem Reuses hdr_gpu_ffi.cu (includes hdr_gpu_jni.cu kernels + JNI compat exports).
rem Same prerequisites as build_jni.bat: CUDA Toolkit 12+ (nvcc), JDK with jni.h, VS build tools.
rem Output: ..\hdr_gpu_ffi.dll (backend/cuda/hdr_gpu_ffi.dll)
setlocal

rem ---- locate JDK (with include\jni.h) ----
set "JDK="
if not "%JAVA_HOME%"=="" if exist "%JAVA_HOME%\include\jni.h" set "JDK=%JAVA_HOME%"
if "%JDK%"=="" if exist "C:\Users\Administrator\.gradle\jdks\jetbrains_s_r_o_-21-amd64-windows.2\include\jni.h" set "JDK=C:\Users\Administrator\.gradle\jdks\jetbrains_s_r_o_-21-amd64-windows.2"
if "%JDK%"=="" if exist "C:\Program Files\Java\jdk-25\include\jni.h" set "JDK=C:\Program Files\Java\jdk-25"
if "%JDK%"=="" (
    echo [ERROR] JDK with jni.h not found. Set JAVA_HOME to a full JDK.
    exit /b 1
)

where nvcc >nul 2>nul
if errorlevel 1 (
    echo [ERROR] nvcc not found. Install CUDA Toolkit and add to PATH.
    exit /b 1
)

set SRC=%~dp0hdr_gpu_ffi.cu
set OUT=%~dp0..\hdr_gpu_ffi.dll

echo [INFO] JDK=%JDK%

rem ---- locate Visual Studio vcvarsall.bat ----
set "VCVARS="
if exist "D:\Program Files\Visual Studio\VC\Auxiliary\Build\vcvarsall.bat" set "VCVARS=D:\Program Files\Visual Studio\VC\Auxiliary\Build\vcvarsall.bat"
if "%VCVARS%"=="" if exist "C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvars64.bat" set "VCVARS=C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvars64.bat"
if "%VCVARS%"=="" if exist "C:\Program Files\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat" set "VCVARS=C:\Program Files\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat"
if "%VCVARS%"=="" (
    echo [ERROR] Visual Studio vcvars not found. Edit this script to set VCVARS.
    exit /b 1
)

echo [INFO] vcvars=%VCVARS%
echo [INFO] nvcc --version:
nvcc --version | findstr /b "Cuda compilation tools"

rem initialize MSVC x64 env (nvcc needs cl.exe)
call "%VCVARS%" x64 >nul
if errorlevel 1 (
    echo [ERROR] vcvarsall init failed
    exit /b 1
)

echo [BUILD] nvcc -shared ...
nvcc -shared -O3 -gencode arch=compute_75,code=compute_75 -cudart=static ^
    -I"%JDK%\include" -I"%JDK%\include\win32" ^
    -Xcompiler "/MD /wd4819" "%SRC%" -o "%OUT%"

if errorlevel 1 (
    echo [ERROR] build failed
    exit /b 1
)

echo [OK] generated %OUT%
endlocal