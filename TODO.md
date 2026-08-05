# TODO（2026-08-05 已完成）

## ✅ 批量处理功能
- Kotlin 后端新增 `/batch/convert`（逐文件返回结果）、`/batch/progress`、`/status`
- 全局信号量 `ConversionSemaphore`：最大并发 = 核心数/2 + 1（本机 16 核 → 9），单张/批量共用
- Electron 主进程 `batch-convert-images` IPC + 轮询 `/batch/progress` 转发；`select-input-images` 多选
- 前端新增「🗂 批量转换」按钮 + 批量队列面板（逐文件状态 + 总体进度条）
- 验证：`node tests/batch_test.js`

## ✅ CUDA 加速
- 新增 `backend/cuda/jni/hdr_gpu_jni.cu`（JNI 桥 + CUDA 内核）+ `build_jni.bat`
- 加速点：Display-P3 转换、增益图（含 min/max 归约）、HDR 变换（PNG 路径）
- Kotlin `HdrGpuJni.kt` 自动加载 DLL，GPU 不可用回退 CPU；`HDR_GPU_DISABLE=1` 强制 CPU
- `/backend`、`/status` 上报 `method=cuda`；输出与 CPU 主图像像素逐字节一致
- 本机已验证：RTX 4060 + CUDA 13.2 + VS（D:\Program Files\Visual Studio）
- 验证：`node tests/gpu_cpu_consistency.js`

## ✅ 优化项目结构
- ICC 移到 `assets/`（2020_profile.icc / display_p3_gainmap.icc / display_p3_primary.icc）
- Kotlin 后端移到 `backend/kotlin/`；CUDA 模块在 `backend/cuda/`
- 更新 main.js / backend_test_util.js / Main.kt（ICC 路径）/ package.json 打包清单

## ✅ 测试代码归档
- 测试脚本统一移到 `tests/`：backend_test_util / verify_ultrahdr / roundtrip_test / check_structure / compare_structure / batch_test / gpu_cpu_consistency

## ✅ 清理无用代码
- 删除：tmp_verify.js、test_backend.js、test_e2e_convert.js、test_python.py、check_ascii.js、check_mpf.js、verify_fixed.js（均被规范脚本替代或引用已删模块）

## 备注
- JDK 21 可用路径：`D:\Program Files\Android Studio\jbr`（无 jni.h）或 `C:\Users\Administrator\.gradle\jdks\jetbrains_s_r_o_-21-amd64-windows.2`
- 清理 java 进程只按命令行匹配 `hdr-converter-backend`，勿误杀 VS Code Java 插件（redhat.java）