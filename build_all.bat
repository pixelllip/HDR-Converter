@echo off
title HDR Convert Build

cd /d "%~dp0"

echo === HDR Convert - Build ===
echo.

:: Clean
echo [Clean]...
if exist build\windows rmdir /s /q build\windows >nul 2>&1
if exist build\web rmdir /s /q build\web >nul 2>&1
echo Done
echo.

:: Web (first, no native deps needed)
echo [1/2] Building Web...
echo.
echo   Usage: build_all.bat deploy  (for GitHub Pages)
echo   Or:    build_all.bat         (for local test)
echo.
if /I "%1"=="deploy" (
    echo   Mode: GitHub Pages (--base-href /HDR-Converter/)
    echo.
    call flutter build web --release --base-href /HDR-Converter/
) else (
    echo   Mode: Local test (--base-href /)
    echo.
    call flutter build web --release
)
if %errorlevel% neq 0 (
    echo Web build failed!
    pause
    exit /b %errorlevel%
)
echo Web build OK
echo   Output: build\web\
echo.

:: Windows (needs network for native deps)
echo [2/2] Building Windows...
echo.
echo Using pure Dart image package (no native deps needed).
echo.
call flutter build windows --release
if %errorlevel% neq 0 (
    echo Windows build failed!
    echo Possible cause: network timeout downloading native dependencies.
    pause
    exit /b %errorlevel%
)
echo Windows build OK
echo   Output: build\windows\x64\runner\Release\
echo.

echo === All done ===
echo   Web:     build\web\
echo   Windows: build\windows\x64\runner\Release\
echo.
pause
echo.

:: 完成
echo ========================================
echo   全部构建完成！
echo ========================================
echo.
echo 📦 Windows: build\windows\x64\runner\Release\
echo 🌐 Web:     build\web\
echo.
echo Web 端部署到 GitHub Pages:
echo   - 将 build\web\ 目录下所有文件推送到 gh-pages 分支
echo   - 或在 GitHub 仓库 Settings ^> Pages 中设为 /docs 目录
echo.
pause
