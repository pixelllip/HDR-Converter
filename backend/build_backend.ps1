# build_backend.ps1 - one-click build of the Kotlin backend jar
#
# Fixes two common build failures:
#   1. System JAVA_HOME may point to a JDK incompatible with Gradle/Kotlin (e.g. JDK 25)
#      -> auto-detect a JDK 17-21
#   2. Gradle is not installed on PATH
#      -> use the project Gradle Wrapper (backend/kotlin/gradlew.bat, pinned 8.14)
#
# Usage:
#   powershell -NoProfile -ExecutionPolicy Bypass -File backend\build_backend.ps1 [extra gradle args...]
#   or simply run build_backend.bat at the project root.
#   Example: build_backend.ps1 --no-daemon -Dorg.gradle.jvmargs="-Xmx1536m -Djava.io.tmpdir=D:\tmp"
#
# NOTE: keep this file pure ASCII (PowerShell 5.1 reads BOM-less UTF-8 as ANSI and
#       Chinese text will corrupt parsing).
# Note: diagnostics are appended to backend\build_diag.log so failures stay visible
#       even if the console scrolls too fast to read.
param(
    # Optional: pin the JDK root explicitly (skips auto-detection).
    # Usable as: powershell -File build_backend.ps1 -JdkHome "D:\Program Files\Android Studio\jbr"
    # or via env var HDR_JDK_HOME. Kept separate so remaining args still forward to gradle.
    [string]$JdkHome = $env:HDR_JDK_HOME,
    # Optional: override the Gradle user HOME (cache/daemon location). Points to the
    # default `%USERPROFILE%\.gradle` unless HDR_GRADLE_HOME is set. Useful when the
    # default .gradle is locked/read-only or you want an isolated cache.
    [string]$GradleUserHome = $env:HDR_GRADLE_HOME,
    # Extra args forwarded verbatim to gradlew (must be the LAST param because it uses
    # ValueFromRemainingArguments). Example: build_backend.ps1 -JdkHome X --no-daemon
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$GradleArgs
)
$ErrorActionPreference = 'Stop'

$backendDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$kotlinDir = Join-Path $backendDir 'kotlin'

# Diagnostics are ALSO appended to this log so a fast-scrolling console never hides the
# real build failure. Only created when a diagnostic line is written.
$diagLog = Join-Path $backendDir 'build_diag.log'
function Write-Diag([string]$msg) {
    $line = ('[{0}] {1}' -f (Get-Date -Format 'HH:mm:ss'), $msg)
    try {
        Add-Content -LiteralPath $diagLog -Value $line -Encoding UTF8
    } catch { /* ignore */ }
    Write-Host $line -ForegroundColor DarkGray
}

# Parse major version from `java -version` ("21.0.11" -> 21, "1.8.0_201" -> 8). 0 = unavailable.
# Version text goes to stderr; PS 5.1 turns piped/redirected native stderr into ErrorRecords and
# with $ErrorActionPreference=Stop the redirect itself throws. So do the redirect inside cmd.exe.
function Get-JavaMajorVersion([string]$javaExe) {
    if (-not (Test-Path -LiteralPath $javaExe)) { return 0 }
    try {
        $verFile = Join-Path $kotlinDir 'build\java_version_probe.txt'
        $verDir = Split-Path -Parent $verFile
        if (-not (Test-Path -LiteralPath $verDir)) { New-Item -ItemType Directory -Path $verDir | Out-Null }
        $cmdLine = '"' + $javaExe + '" -version 2> "' + $verFile + '"'
        cmd /d /c $cmdLine
        $line = Get-Content -LiteralPath $verFile -ErrorAction SilentlyContinue | Select-Object -First 1
        Remove-Item -LiteralPath $verFile -ErrorAction SilentlyContinue
        if ($line -match '"(\d+)(?:\.(\d+))?') {
            $major = [int]$Matches[1]
            if ($major -eq 1) { return [int]$Matches[2] }   # 1.8 -> 8
            return $major
        }
    } catch { }
    return 0
}

# ---------- 1. detect JDK 17-21 ----------
# When HDR_JDK_HOME / -JdkHome is set, trust that JDK root directly (skips
# auto-detection, which can mis-detect in some environments). Works with the
# Android Studio JBR or any JDK 17-21.
if ($JdkHome) {
    if (-not (Test-Path -LiteralPath (Join-Path $JdkHome 'bin\java.exe'))) {
        $err = "[build_backend] JdkHome has no bin\java.exe: $JdkHome"
        Write-Diag $err
        throw $err
    }
    $chosen = Join-Path $JdkHome 'bin\java.exe'
    $chosenMajor = Get-JavaMajorVersion $chosen
    Write-Diag "Explicit JDK: $chosen (major=$chosenMajor)"
} else {
    $candidates = New-Object System.Collections.Generic.List[string]
    if ($env:JAVA_HOME) { $candidates.Add((Join-Path $env:JAVA_HOME 'bin\java.exe')) }
    $candidates.Add((Join-Path $env:USERPROFILE '.gradle\jdks\jetbrains_s_r_o_-21-amd64-windows.2\bin\java.exe'))
    $candidates.Add((Join-Path $env:USERPROFILE '.gradle\jdks\amazon-corretto-21-*\bin\java.exe'))
    $candidates.Add('D:\Program Files\Android Studio\jbr\bin\java.exe')
    $candidates.Add('C:\Program Files\Android\Android Studio\jbr\bin\java.exe')
    $candidates.Add('C:\Program Files\Java\jdk-21\bin\java.exe')
    $candidates.Add('C:\Program Files\Java\jdk-17\bin\java.exe')
    # scan every JDK under C:\Program Files\Java
    $javaRoot = 'C:\Program Files\Java'
    if (Test-Path $javaRoot) {
        Get-ChildItem $javaRoot -Directory -ErrorAction SilentlyContinue | ForEach-Object {
            $candidates.Add((Join-Path $_.FullName 'bin\java.exe'))
        }
    }

    $chosen = $null
    $chosenMajor = 0
    foreach ($pattern in $candidates) {
        $paths = @()
        if ($pattern -match '[*?]') {
            $paths = @(Get-ChildItem $pattern -ErrorAction SilentlyContinue | Select-Object -ExpandProperty FullName)
        } elseif (Test-Path -LiteralPath $pattern) {
            $paths = @($pattern)
        }
        foreach ($p in $paths) {
            $maj = Get-JavaMajorVersion $p
            Write-Diag "Candidate $p -> major=$maj"
            if ($maj -ge 17 -and $maj -le 21) { $chosen = $p; $chosenMajor = $maj; break }
        }
        if ($chosen) { break }
    }
}

if (-not $chosen) {
    $err = '[build_backend] No JDK 17-21 found. Set env HDR_JDK_HOME to a JDK 17-21 root (e.g. D:\Program Files\Android Studio\jbr) or install JDK 21.'
    Write-Diag $err
    Write-Host $err -ForegroundColor Red
    exit 1
}
$env:JAVA_HOME = Split-Path -Parent (Split-Path -Parent $chosen)
Write-Host ("[build_backend] Using JDK {0}: {1}" -f $chosenMajor, $env:JAVA_HOME) -ForegroundColor Cyan
Write-Diag "Selected JAVA_HOME=$env:JAVA_HOME"

# ---------- 2. build with the Gradle Wrapper ----------
$gradlew = Join-Path $kotlinDir 'gradlew.bat'
if (-not (Test-Path -LiteralPath $gradlew)) {
    Write-Host "[build_backend] Gradle Wrapper not found: $gradlew" -ForegroundColor Red
    Write-Host 'Generate it once with a local gradle: cd backend/kotlin; gradle wrapper --gradle-version 8.14'
    exit 1
}

Push-Location $kotlinDir
try {
    $gradleEnv = @{}
    if ($GradleUserHome) {
        # 让当前进程（及其派生的 gradle daemon Java）继承 GRADLE_USER_HOME 覆盖
        $env:GRADLE_USER_HOME = $GradleUserHome
        Write-Diag "GRADLE_USER_HOME=$GradleUserHome"
        Write-Host "[build_backend] GRADLE_USER_HOME -> $GradleUserHome" -ForegroundColor Yellow
    }
    if ($GradleArgs -and $GradleArgs.Count -gt 0) {
        & $gradlew --console=plain @GradleArgs jar
    } else {
        & $gradlew --console=plain jar
    }
    $code = $LASTEXITCODE
    if ($code -eq 0) {
        $jar = Join-Path $kotlinDir 'build\libs\hdr-converter-backend.jar'
        Write-Host "[build_backend] Build OK: $jar" -ForegroundColor Green
    } else {
        Write-Diag "Gradle 构建退出码=$code（详见上方输出）"
    }
    exit $code
} finally {
    Pop-Location
}
