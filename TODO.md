# TODO（2026-08-12 已完成视频支持）


## ✅ 后端生命周期修复（2026-08-13，portable 版孤儿 JVM）
- **问题**：每次启动页面加载即调 `get-backend-status` → 必然 spawn 后端；而 `spawn('java')` 解析到 Oracle javapath 启动器（会再拉一个真实 JVM 子进程），`child.kill()` 只作用于启动器壳；JVM 端 `start(wait=true)` 永久阻塞无自终止 → 正常退出/崩溃都会遗留孤儿 JVM（实测发现 2 个：18765 + 随机端口）。
- **修复**（main.js + Main.kt）：
  - `resolveJavaExecutable()`：优先 `JAVA_HOME\bin\java.exe`，跳过 PATH 中的 javapath → 直接单进程 spawn，stdin 管道直达 JVM
  - `will-quit` 改用 `taskkill /PID /T /F` 杀进程树（child.kill() 在 Windows 上不可靠）
  - `Main.kt` 新增 stdin EOF 监视线程：Electron 退出/崩溃 → 管道关闭 → JVM 自动 `server.stop()` + 退出（stdin 无效时自动跳过，不影响测试等场景）
  - `app.requestSingleInstanceLock()` 单实例锁：重复启动只聚焦已有窗口，不叠加后端
  - 启动时 `sweepOrphanBackends()` 清理历史遗留的 `hdr-converter-backend` java 进程（在 createWindow 前 await，避免误杀本实例后端）
- **验证**：单 java 进程（无 stub 壳）、重复启动不叠加、强杀 App 后 JVM stdin EOF 自终止、正常关闭窗口后端被 taskkill 回收、sweep 命令可杀残留后端

## ✅ 视频 HDR 转换（ffmpeg 接入，2026-08-12）
- **两条链路均可转 SDR 视频 → HDR10**（HEVC 10bit / bt2020 / smpte2084 / master-display+MaxCLL）：
  - **链路 1（直接滤镜）**：一条 ffmpeg 滤镜链（setparams → zscale 线性化 → npl 提亮 → libx265），快速全局提亮
  - **链路 2（逐帧增益图）**：拆帧 → Kotlin 后端 `/video-frame` 逐帧重建线性 HDR（增益图高光扩展，保中间调）→ 16-bit PAM → ffmpeg 编码 → 合并原音频
- **Kotlin 后端新增**：`UltraHdrEncoder.reconstructLinearHdrFrame`（sRGB→线性 → 高光掩膜增益 → 16-bit PAM）+ Main.kt `POST /video-frame`（Models.kt 加 VideoFrameRequest/Response）
  - 复用 computeGainMap 的 highlightStart=0.5 / 保色调模型，输出 P7 PAM 大端 RGB
  - 验证：`node tests/verify_video_frame.js`（2560x1440 帧 605ms，高光 ×5.28，中间调保色）
- **video_converter.js**（主进程 ffmpeg 封装）：probe / 链路1 / 链路2 / 预览 / 取帧；`-progress pipe:1` 解析进度；taskkill 取消
- **前端重构**：首页（拖入图片/视频自动识别类型）+ 图片子界面 + 视频子界面（两条链路参数/进度/取消）；「🏠 首页」返回
- **HDR 视频预览**：Chromium 播不了 HEVC HDR → 转换后可生成色调映射回 SDR 的预览 MP4（`<video>` 可拖动）；另有拖动条 + `-ss` 按需取帧精确渲染任意帧
- 验证：`node tests/verify_video_convert.js`（合成 SDR 视频 → 两链路 HDR10 → 元数据 yuv420p10le/bt2020/smpte2084 + 峰值亮度 链路2 ×5.8）
- 踩坑：`#viewImage/#viewVideo` 包裹后必须设为 flex column 填满视口，否则 `#main` 的 flex:1 失效布局塌陷；终端 cwd 可能残留需用绝对路径

## ✅ 批量处理功能（2026-08-05）
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