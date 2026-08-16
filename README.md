# HDR Converter Electron

将普通 SDR（标准动态范围）图片 / 视频转换为 HDR（高动态范围）的 Electron 桌面应用。

## 运行方式

```bash
cd hdr_electron
npm install
npm start
```

> 图片转换由 **Kotlin 后端**（JVM，多线程并行增益图 + 可选 CUDA）完成，JS 后端已移除。首次转换或加载页面时主进程自动启动 `backend/kotlin/build/libs/hdr-converter-backend.jar`（Ktor HTTP 服务，自动选端口），退出应用时自动关闭。若 JAR 不存在，需先构建：

```bash
build_backend.bat          # 一键构建：自动探测 JDK 17~21 + 项目自带 Gradle Wrapper
# 或手动：
cd backend/kotlin
gradlew.bat jar            # Gradle Wrapper（8.14），无需系统安装 gradle
```

> `backend/build_backend.ps1` 会自动挑选 JDK 17~21（优先 `JAVA_HOME`，其次常见安装位置如 `~/.gradle/jdks`、Android Studio JBR、`C:\Program Files\Java`），避免系统默认 JDK（如 25）与 Gradle/Kotlin 不兼容导致构建失败；需要额外 Gradle 参数时透传即可，如 `build_backend.ps1 --no-daemon`。

> 视频转换由 **ffmpeg 9.0 essentials**（`backend/ffmpeg/ffmpeg.exe` + `ffprobe.exe`，约 196MB，含 libx265 / hevc_nvenc / zscale 等所需组件）完成，无需额外安装。

## 打包发布

```bash
npm run dist    # electron-builder --win portable
```

产物：`dist/HDR-Converter-1.0.0.exe`（portable 单文件，约 136MB）；`dist/win-unpacked/` 为解包调试目录。

> 打包关键点：`build.files` 含 `backend/ffmpeg/**`，且 `asarUnpack` 必须包含 `backend/ffmpeg/**`、`backend/cuda/**`、`backend/kotlin/build/libs/*.jar`——这些文件被外部进程（ffmpeg / JVM System.load / java -jar）读取，必须解包在 asar 之外，否则打包后运行会找不到。

## 功能

- **首页 + 子界面**：启动进入首页，可拖入图片或视频（也可点击「选择图片 / 选择视频」），按文件类型**自动跳转到图片或视频子界面**；两个子界面均有「🏠 首页」返回按钮（返回时释放预览资源；**转换过程中返回按钮锁定**）
- **拖拽选择文件**：首页 / 图片 / 视频子界面均可拖入文件（Electron 33 移除了 `File.path`，通过 `webUtils.getPathForFile` 取路径修复）
- **图片 → HDR**（HDR PNG / **HDR JPEG（ICC 增益，BT.2020）** / **Ultra HDR JPEG**）
- **Ultra HDR JPEG** 符合 Android Ultra HDR 图像格式（增益图 + MPF + GContainer/hdrgm XMP + ICC）
- **视频 → HDR10**（HEVC 10-bit / BT.2020 / PQ / master-display + MaxCLL，ffmpeg），两种链路 + **编码器可选**：
  - **编码器**（默认 **GPU · NVENC**，NVIDIA 硬件编码、快、释放 CPU；无 NVIDIA 时自动回退 CPU x265）：`GPU · NVENC` 或 `CPU · x265`（质量更好、压缩率高）；输出同样注入完整 HDR 元数据
  - **直接转 · 单层色调映射**（对应图片「**ICC 增益 jpg_icc**」）：Kotlin 后端逐帧 `applyHdrTransform`（无自动伽马，避免闪烁）→ 16-bit PAM → ffmpeg 编码 HDR10
  - **精确 · 逐帧增益图**（对应图片「**Ultra HDR jpg**」增益图双层）：拆帧 → Kotlin 后端 `/video-frame` 逐帧重建（增益图高光扩展，只提亮高光、保中间调）→ 16-bit PAM → ffmpeg 编码 HDR10（可设处理宽度上限省内存）
  - **两链路支持全套图片参数**：**峰值亮度（尼特）**（内部按 EV=log2(峰值/白点) 换算：直接转曝光=峰值/白点(2^EV)（SDR 白提到峰值，**微调明暗已移除**——乘 <1 会把 HDR 压暗）、增益图 maxBoost=2^EV）、伽马；**RGB 通道仅直接转显示/生效**（jpg_icc 式）；**Ultra HDR 式增益图不显示也不应用 RGB**（与图片 Ultra HDR 一致，底图=原图，避免视频被压暗），由 Kotlin float64 计算不裁剪
  - **白点用户可调**（视频 + 图片预览，默认 **203** 尼特，BT.2408）；**峰值亮度（尼特）滑块**（**范围固定 400~1250 尼特**，默认 **574** = 203×2^1.5EV，图片/视频统一）统一控制高光上限：视频 MaxCLL/NPL/PAM 峰值、图片 Ultra HDR 增益图 `maxBoost` 随之联动
- **视频预览**：预览排版与图片一致（宽幅上下排列 / 标准左右并排）；左侧**直接播放原视频**（不截单帧），右侧**首帧 HDR 预览**（用图片 HDR 链路 `/preview` 处理首帧，direct 模式对应 jpg_icc、frames 模式对应 Ultra HDR，随模式/全套参数实时刷新）；**转换完成后全片预览**——直接把 HDR 输出加载进 `<video>` 播放（已声明完整 HDR 元数据 + Electron 内置软件 HEVC 解码，Chromium 识别为 HDR 视频；HDR 屏直接 HDR 渲染、SDR 屏自动 tone map），原生进度条可拖动查看任意帧
- **图片批量转换**：多选 / 文件夹导入，按队列转换，最大并发 = 核心数/2 + 1，逐文件状态 + 总体进度条
- **CUDA 加速**（图片链路）：Display-P3 转换 / 增益图 / HDR 变换走 GPU（RTX 4060 等），不可用自动回退 CPU
- 实时转换进度条、实时参数调节（峰值亮度（尼特） / 明暗 / 伽马 / RGB）、SDR/HDR 对比预览

## 项目结构

```
hdr_electron/
├── assets/
│   ├── display_p3_primary.icc   # Display-P3 + sRGB 传递（主图像）
│   ├── display_p3_gainmap.icc   # sRGB 增益图 ICC
│   └── 2020_profile.icc     # BT.2020 ICC 配置文件（PNG 用）
├── backend/
│   ├── build_backend.ps1   # 后端一键构建（自动探测 JDK 17~21 + Gradle Wrapper）
│   ├── cuda/               # GPU 加速（CUDA JNI DLL + 内核）
│   ├── ffmpeg/             # ffmpeg 9.0 essentials（ffmpeg / ffprobe，~196MB；ffplay 已删）
│   └── kotlin/
│       ├── gradlew(.bat)   # Gradle Wrapper（8.14，构建后端用）
│       └── src/main/kotlin/com/hdrconverter/
│           ├── UltraHdrEncoder.kt   # Ultra HDR 编码器 + 视频逐帧重建（/video-frame）
│           ├── HdrConverter.kt      # 像素变换核心
│           ├── HdrGpuJni.kt         # CUDA JNI 桥（GPU 不可用时回退 CPU）
│           ├── IccInjector.kt       # ICC 注入
│           └── Main.kt              # Ktor HTTP 服务（/convert /batch/convert /video-frame /progress ...）
├── video_converter.js      # 视频转换（ffmpeg 封装）：两条链路 + 取消 + 进度
├── mp4_hdr.js              # 向 MP4 容器注入 mdcv/clli HDR 元数据盒（Chromium 依赖）
├── extracted_hdr_core/      # HDR 核心 JS 库（可复用，含规范解码）
├── tests/                   # 验证与测试脚本
├── main.js                  # Electron 主进程（后端管理 + IPC + 进度轮询 + 视频转换）
├── preload.js               # Electron 预加载脚本
├── views/                  # 前端 UI（Material 3）：home / image / video 三个独立界面 + 共享 md3.css/md3.js
│   ├── home.html           #   首页（拖入图片/视频自动识别类型 → 跳转对应界面）
│   ├── image.html          #   图片视图（输入输出 / 参数质量 / 转换队列）
│   ├── video.html          #   视频视图（两条链路 / 编码与性能）
│   ├── md3.css             #   共享样式（MD3 调色板，深浅色 + 跟随系统主题色）
│   └── md3.js              #   共享脚本（图标雪碧图 / 调色板生成 / 系统主题接线）
└── package.json             # Node.js 依赖
```

## 技术原理（视频）

- **链路 1（直接滤镜）**：`setparams → zscale 线性化(npl=100) → zscale 到 bt2020/PQ(npl=目标峰值) → libx265 10bit`。SDR 白点（≈100 尼特）被提升到目标峰值（默认 400 尼特），全局提亮
- **链路 2（逐帧增益图）**：ffmpeg 解码 SDR 视频为 PNG 帧 → 每帧 POST Kotlin `/video-frame`（sRGB→线性 → 增益图高光扩展 `1+(maxBoost-1)·mask^γ`，50% 亮度以下 gain=1 严格保色调）→ 16-bit PAM（大端 RGB，归一化峰值 8.0）→ ffmpeg 编码 HDR10（`zscale pin=bt709 → bt2020/PQ，npl=800`）→ 合并原音频
- **HDR 预览**：转换后直接把 HDR 输出加载进 `<video>` 播放（Electron 内置软件 HEVC 解码 + Chromium HDR 识别），原生进度条拖动查看任意帧；**不再生成 SDR tone-map 预览**（Chromium 会自动 tone map 到 SDR 屏 / 直接渲染到 HDR 屏）
- **HDR 元数据声明（Chromium 识别 HDR 的关键）**：ffmpeg/libx265 只写 `colr` 盒 + 码流 SEI，**不写 `mdcv`/`clli` 容器盒**。Chromium 的 MP4 解析器从这三个盒读 HDR 元数据 → 转换后由 `mp4_hdr.js` 把 `mdcv`（P3 主色 mastering display）和 `clli`（MaxCLL/MaxFALL）注入视频采样描述（`stsd → hvc1` 内），并向上传播祖先盒大小；编码参数同时声明 `repeat-headers=1`、`profile=main10`、输出 `-color_primaries bt2020 -color_trc smpte2084 -colorspace bt2020nc -color_range tv`
- 两条链路输出均带：Main10 + `hvc1` + `bt2020/smpte2084/bt2020nc/tv` + 容器 `colr/mdcv/clli` 盒 + 流 side data（mastering display + content light level）+ 码流 SEI → **Chromium 可识别为 HDR 视频**

## GPU / CUDA 加速（图片链路，已接入）

- `backend/cuda/jni/hdr_gpu_jni.cu` —— JNI 桥 + CUDA 内核（Runtime API，PTX compute_75 嵌入，驱动 JIT）
- 加速点：**Display-P3 转换**（主图像）、**增益图计算**、**HDR 变换**（PNG 路径）
- Kotlin 端 `HdrGpuJni.kt` 通过 `System.load` 加载 `backend/cuda/hdr_gpu_jni.dll`；GPU 不可用/失败时自动回退 CPU 多线程
- 后端 `/backend` 接口上报 `method=cuda`；可设环境变量 `HDR_GPU_DISABLE=1` 强制 CPU
- 一致性：GPU（float32）与 CPU（float64）主图像像素逐字节一致；增益图个别边界像素差 ≤1 LSB

## 验证脚本

- `node tests/verify_ultrahdr.js` —— 校验输出 JPG 的 XMP / ICC / MPF / 增益图结构
- `node tests/roundtrip_test.js` —— 端到端闭环：编码 → 提取 → 按规范公式重建 HDR
- `node tests/verify_video_frame.js` —— `/video-frame` 端点（中间调保色 + 高光扩展）
- `node tests/verify_video_convert.js` —— 视频两条链路端到端（合成 SDR 视频 → 两条链路转 HDR10 → 元数据 + 峰值亮度验证）
- `node tests/verify_video_nvenc.js` —— 视频两条链路用 **GPU NVENC** 编码 → HDR 元数据验证
- `node tests/verify_video_firstframe_preview.js` —— 视频首帧提取 + 图片 HDR 链路预览（jpg_icc / Ultra HDR）
- `node tests/verify_video_rgb.js` —— 视频 RGB 通道（直接转生效 / Ultra HDR 式忽略）
- `node tests/verify_hdr_metadata.js [file]` —— 校验 HDR 元数据声明（Main10/hvc1、色彩属性、容器 colr/mdcv/clli 盒、流 side data、码流 SEI）
- `node tests/check_structure.js <file>` / `compare_structure.js` / `batch_test.js` / `gpu_cpu_consistency.js` / `jpg_icc_test.js`

