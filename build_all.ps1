param(
    [switch]$SkipWindows,
    [switch]$SkipWeb,
    [switch]$DeployToGitHub
)

$ErrorActionPreference = "Continue"

$rootDir = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location $rootDir

Write-Host "=== HDR Convert - Build ===" -ForegroundColor Cyan
Write-Host ""

# Clean
if (-not ($SkipWindows -and $SkipWeb)) {
    Write-Host "[Clean]..." -ForegroundColor Yellow
    if (Test-Path "build\windows") { Remove-Item -Recurse -Force "build\windows" -ErrorAction SilentlyContinue }
    if (Test-Path "build\web") { Remove-Item -Recurse -Force "build\web" -ErrorAction SilentlyContinue }
    Write-Host "Done" -ForegroundColor Green
    Write-Host ""
}

# Web
if (-not $SkipWeb) {
    Write-Host "[1/2] Building Web..." -ForegroundColor Yellow
    if ($DeployToGitHub) {
        Write-Host "  Mode: GitHub Pages (--base-href /HDR-Converter/)" -ForegroundColor Magenta
        & flutter build web --release --base-href /HDR-Converter/
    }
    else {
        Write-Host "  Mode: Local test (--base-href /)" -ForegroundColor Magenta
        & flutter build web --release
    }
    if ($LASTEXITCODE -ne 0) {
        Write-Host "Web build failed!" -ForegroundColor Red
        Read-Host "Press Enter to exit"
        exit $LASTEXITCODE
    }
    Write-Host "Web build OK" -ForegroundColor Green
    Write-Host "  Output: build\web\"
    Write-Host ""
}

# Windows
if (-not $SkipWindows) {
    Write-Host "[2/2] Building Windows..." -ForegroundColor Yellow

    # 可选: 编译 GPU 着色器
    $shaderScript = "windows\plugins\hdr_gpu\shaders\compile_shaders.bat"
    if (Test-Path $shaderScript) {
        Write-Host "  Compiling GPU shaders..." -ForegroundColor Magenta
        Push-Location "windows\plugins\hdr_gpu\shaders"
        & ".\compile_shaders.bat"
        Pop-Location
    }

    # 可选: 编译 CUDA 内核 (需要 CUDA Toolkit)
    $cudaScript = "windows\plugins\hdr_gpu\compile_cuda.bat"
    if (Test-Path $cudaScript) {
        Write-Host "  Compiling CUDA kernels..." -ForegroundColor Magenta
        Push-Location "windows\plugins\hdr_gpu"
        & ".\compile_cuda.bat"
        Pop-Location
    }

    Write-Host "  Building with GPU support (DirectCompute + optional CUDA)..."
    & flutter build windows --release
    if ($LASTEXITCODE -ne 0) {
        Write-Host "Windows build failed!" -ForegroundColor Red
        Write-Host "  Possible causes: network timeout downloading native dependencies."
        Write-Host "  Try: .\build_all.ps1 -SkipWindows (web only)"
        Read-Host "Press Enter to exit"
        exit $LASTEXITCODE
    }
    Write-Host "Windows build OK" -ForegroundColor Green
    Write-Host "  Output: build\windows\x64\runner\Release\"
    Write-Host ""
}

Write-Host "=== All done ===" -ForegroundColor Cyan
if (-not $SkipWeb) { Write-Host "  Web:     build\web\" }
if (-not $SkipWindows) { Write-Host "  Windows: build\windows\x64\runner\Release\" }
Write-Host ""

Read-Host "Press Enter to exit"
