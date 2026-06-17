# 手动下载 image_magick_q8_hdri 构建所需的依赖
# 解决 CMake FetchContent 在部分网络环境下 SSL 连接重置的问题

$depsDir = "build\windows\x64\_deps"
$rootDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$baseUrl = "https://github.com/Haidar0096/image_magick_ffi_deps/raw/master"

# 三个需要下载的依赖
$deps = @(
    @{ name = "dart_sdk_api"; url = "$baseUrl/dart_sdk_api.zip" },
    @{ name = "json-c"; url = "$baseUrl/json-c/windows.zip" },
    @{ name = "imagemagick-windows"; url = "$baseUrl/ImageMagick/x64/Q8-HDRI.zip" }
)

Write-Host "=== Downloading build dependencies ===" -ForegroundColor Cyan
Write-Host ""

Set-Location $rootDir

# 先试 SSL 验证关闭的环境变量
$env:CMAKE_TLS_VERIFY = "0"

foreach ($dep in $deps) {
    $name = $dep.name
    $url = $dep.url
    $zip = "$depsDir/$name.zip"
    $srcDir = "$depsDir/$name-src"

    if (Test-Path $srcDir) {
        Write-Host "[SKIP] $name - already exists at $srcDir" -ForegroundColor Green
        continue
    }

    Write-Host "[DL] $name..." -ForegroundColor Yellow
    Write-Host "  from: $url"

    # 确保目录存在
    New-Item -ItemType Directory -Force -Path $depsDir | Out-Null

    # 下载
    try {
        Invoke-WebRequest -Uri $url -OutFile $zip -UseBasicParsing -ErrorAction Stop
        Write-Host "  downloaded OK" -ForegroundColor Green
    }
    catch {
        Write-Host "  download failed: $_" -ForegroundColor Red
        Write-Host "  Trying with curl (bypass SSL verify)..." -ForegroundColor Yellow
        # 用 curl 再试
        & curl -k -L -o $zip $url 2>&1
        if ($LASTEXITCODE -ne 0) {
            Write-Host "  curl also failed. Please download manually:" -ForegroundColor Red
            Write-Host "  $url" -ForegroundColor White
            continue
        }
    }

    # 解压到目标目录
    try {
        Expand-Archive -Path $zip -DestinationPath $srcDir -Force -ErrorAction Stop
        Write-Host "  extracted to $srcDir" -ForegroundColor Green
    }
    catch {
        Write-Host "  extract failed: $_" -ForegroundColor Red
    }

    # 清理zip
    Remove-Item $zip -Force -ErrorAction SilentlyContinue
}

Write-Host ""
Write-Host "=== Done ===" -ForegroundColor Cyan
Write-Host ""
Write-Host "Now run: .\build_all.ps1" -ForegroundColor White
Read-Host "Press Enter to exit"
