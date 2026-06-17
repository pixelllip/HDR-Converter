param(
    [switch]$SkipWindows,
    [switch]$SkipWeb
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
    & flutter build web --release --base-href /
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
    Write-Host "  Note: image_magick_q8_hdri needs to download native deps from GitHub."
    Write-Host "  If it hangs, press Ctrl+C and run with -SkipWindows for web-only build."
    Write-Host ""
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
