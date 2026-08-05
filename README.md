# HDR Converter Electron

将普通 SDR（标准动态范围）图片转换为 HDR（高动态范围）图片的 Electron 桌面应用。

## 运行方式

```bash
cd hdr_electron
npm install
npm start
```

> 转换由 **Kotlin 后端**（JVM，多线程并行增益图 + 可选 CUDA）完成，JS 后端已移除。首次转换或加载页面时主进程自动启动 `backend/kotlin/build/libs/hdr-converter-backend.jar`（Ktor HTTP 服务，自动选端口），退出应用时自动关闭。若 JAR 不存在，需先构建：

```bash
cd backend/kotlin
# 需要 JDK 21（Gradle 8.12 与系统 JDK 不兼容时指定 JAVA_HOME）
$env:JAVA_HOME="D:\Program Files\Android Studio\jbr"
gradle jar
```

## 功能

- SDR → HDR 转换（HDR PNG / **HDR JPEG（ICC 增益，BT.2020）** / **Ultra HDR JPEG**）
- **Ultra HDR JPEG** 符合 Android Ultra HDR 图像格式（增益图 + MPF + GContainer/hdrgm XMP + ICC）
- **HDR JPEG（ICC 增益）**：与 HDR PNG 同方案（自动伽马色调映射），输出 JPEG 并在标准位置（所有前置 APP 段之后）注入 BT.2020 ICC
- **JPEG 质量滑块**（默认 100%）：对 jpg / jpg_icc 输出生效（含 Ultra HDR 主图与增益图）
- **批量转换**：一次选择多张图片 / **整个文件夹导入**，可指定**批量输出文件夹**，按队列转换，最大并发 = 核心数/2 + 1（后端全局信号量限流），逐文件状态展示 + 总体进度条
- **CUDA 加速**：Display-P3 转换 / 增益图 / HDR 变换走 GPU（RTX 4060 等），GPU 不可用时自动回退 CPU 多线程
- **转换前展示转换方式**：CUDA 加速（NVIDIA GPU）或 CPU 多线程（N 核）
- **实时转换进度条**（读取 → Display-P3 → 增益图 → 编码）
- 实时参数调节（HDR 强度、明暗、伽马校正）
- 实时预览 SDR / HDR 对比
- 拖拽 / 点击选择图片

## 项目结构

```
hdr_electron/
├── assets/
│   ├── display_p3_primary.icc   # Display-P3 + sRGB 传递（主图像）
│   ├── display_p3_gainmap.icc   # sRGB 增益图 ICC
│   └── 2020_profile.icc     # BT.2020 ICC 配置文件（PNG 用）
├── backend/
│   ├── cuda/               # GPU 加速（CUDA JNI DLL + 内核）
│   └── kotlin/
│       └── src/main/kotlin/com/hdrconverter/
│           ├── UltraHdrEncoder.kt   # Ultra HDR JPEG 编码器（Kotlin，多线程增益图 + 进度回调）
│           ├── HdrConverter.kt      # 像素变换核心
│           ├── HdrGpu.kt            # CUDA JNI 桥（GPU 不可用时回退 CPU）
│           ├── IccInjector.kt       # ICC 注入
│           └── Main.kt              # Ktor HTTP 服务（/convert /batch/convert /progress /backend /health）
├── extracted_hdr_core/      # HDR 核心 JS 库（可复用，含规范解码）
├── tests/                   # 验证与测试脚本
│   ├── backend_test_util.js # 验证脚本通用辅助（拉起 Kotlin 后端并调用 /convert）
│   ├── verify_ultrahdr.js   # 校验输出 JPG 结构
│   ├── roundtrip_test.js    # 端到端闭环
│   ├── check_structure.js   # 结构检查
│   └── compare_structure.js # 与真实 Google 文件对比
├── main.js                  # Electron 主进程（管理 Kotlin 后端 + IPC + 进度轮询 + 批量队列）
├── preload.js               # Electron 预加载脚本
├── hdr_viewer.html          # 前端 UI（进度条 + 转换方式提示 + 批量队列）
└── package.json             # Node.js 依赖
```

## 技术原理

- **JPG（Ultra HDR）**：主图像 = **原始输入**（像素转为 Display-P3 并标注 Display-P3+sRGB 传递 ICC，保证 SDR 查看器看到原图、绝不泛白）；叠加一张 8-bit 灰度增益图（真正的高光扩展，maxBoost = 2^hdrIntensity，仅高光区域被提亮）；写入 GContainer + hdrgm XMP（对数增益元数据）与 MPF 多图索引；HDR 显示器按规范公式重建高动态范围（高光可达 maxBoost×SDR 白点）
- **PNG / JPG（ICC 增益）**：sRGB → 线性 → HDR 色调映射（自动伽马 + 曝光），PNG 嵌入 iCCP、JPG 在标准位置注入 BT.2020 ICC APP2（所有前置 APP 段之后）
- JPEG 质量：`jpg` / `jpg_icc` 输出的编码质量由质量滑块控制（默认 100%）
- 图像编码：Java ImageIO（Kotlin 后端）

## GPU / CUDA 加速（已接入）

- `backend/cuda/jni/hdr_gpu_jni.cu` —— JNI 桥 + CUDA 内核（Runtime API，PTX compute_75 嵌入，驱动 JIT）
- 加速点：**Display-P3 转换**（主图像）、**增益图计算**（含 min/max 归约）、**HDR 变换**（PNG 路径，含自动伽马归约）
- Kotlin 端 `HdrGpuJni.kt` 通过 `System.load` 加载 `backend/cuda/hdr_gpu_jni.dll`；GPU 不可用/失败时**自动回退 CPU 多线程**，不影响可用性
- 后端 `/backend` 接口上报 `method=cuda`；可设环境变量 `HDR_GPU_DISABLE=1` 强制使用 CPU（用于对比测试）
- 一致性：GPU（float32）与 CPU（float64）主图像像素逐字节一致；增益图个别边界像素差 ≤1 LSB，闭环 HDR 重建一致

### 构建 JNI DLL

```bash
cd backend/cuda/jni
build_jni.bat   # 需要 CUDA Toolkit（nvcc）+ Visual Studio（vcvars）+ JDK（自动探测）
```

### 验证

- `node tests/gpu_cpu_consistency.js` —— GPU/CPU 输出一致性对比

## 验证脚本

- `node tests/verify_ultrahdr.js` —— 校验输出 JPG 的 XMP / ICC / MPF / 增益图结构
- `node tests/roundtrip_test.js` —— 端到端闭环：编码 → 提取 → 按规范公式重建 HDR
- `node tests/check_structure.js <file>` —— 检查单个文件的 Ultra HDR 结构
- `node tests/compare_structure.js` —— 对比我们的输出与真实 Google Ultra HDR 文件的结构
- `node tests/batch_test.js` —— 批量转换：逐文件结果 + 并发容量验证
- `node tests/gpu_cpu_consistency.js` —— GPU/CPU 输出一致性对比（需 CUDA DLL）
- `node tests/jpg_icc_test.js` —— HDR JPEG（ICC 增益）输出：ICC 位置 / 严格段校验 / 质量滑块

