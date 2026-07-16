@echo off
REM 编译 HLSL Compute Shaders 为 CSO (Compiled Shader Object)
REM 需要 Windows SDK 中的 fxc.exe

set FXC=fxc.exe
if exist "%WindowsSdkDir%\bin\x64\fxc.exe" (
    set FXC="%WindowsSdkDir%\bin\x64\fxc.exe"
) else if exist "C:\Program Files (x86)\Windows Kits\10\bin\10.0.22621.0\x64\fxc.exe" (
    set FXC="C:\Program Files (x86)\Windows Kits\10\bin\10.0.22621.0\x64\fxc.exe"
) else if exist "C:\Program Files (x86)\Windows Kits\10\bin\10.0.19041.0\x64\fxc.exe" (
    set FXC="C:\Program Files (x86)\Windows Kits\10\bin\10.0.19041.0\x64\fxc.exe"
) else (
    echo fxc.exe not found. Will use runtime HLSL compilation.
    exit /b 1
)

echo Compiling pass1_srgb_to_linear.hlsl...
%FXC% /T cs_5_0 /E main /Fo pass1_srgb_to_linear.cso pass1_srgb_to_linear.hlsl
if %ERRORLEVEL% neq 0 (
    echo Failed to compile pass1
    exit /b 1
)

echo Compiling pass2_apply_hdr.hlsl...
%FXC% /T cs_5_0 /E main /Fo pass2_apply_hdr.cso pass2_apply_hdr.hlsl
if %ERRORLEVEL% neq 0 (
    echo Failed to compile pass2
    exit /b 1
)

echo Shaders compiled successfully.
