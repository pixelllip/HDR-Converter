@echo off
rem 一键构建 Kotlin 后端 jar（存档版：Kotlin 已停止维护，仅供复现旧产物）
rem 自动探测 JDK 17~21 + Gradle Wrapper，详见同目录 build_backend.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0build_backend.ps1"
exit /b %ERRORLEVEL%