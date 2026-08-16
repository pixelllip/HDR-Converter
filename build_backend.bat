@echo off
rem 一键构建 Kotlin 后端 jar（自动探测 JDK 17~21 + Gradle Wrapper，详见 backend\build_backend.ps1）
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0backend\build_backend.ps1"
exit /b %ERRORLEVEL%
