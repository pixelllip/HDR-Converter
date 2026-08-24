# 决策论证书：Flutter 能否做成 Android 端 HDR 转换？

> 目标：回答"Flutter 在安卓不支持 HDR 显示，所以不能用它做 HDR 转换吗？"
> 方法：把"产出 HDR 文件"与"在 App 内显示 HDR"分开论证，依托本机 Flutter SDK(3.44.2) 源码与已确证的 Android 官方 API。

## 一、先说结论

| 功能 | 是否依赖 Flutter 的 HDR 显示 | Flutter 能不能做 |
|---|---|---|
| **SDR→合规 Ultra HDR JPEG** | ❌ 不依赖 | ✅ 能（计算在 Kotlin/官方 API） |
| **SDR/Ultra HDR→HDR 视频(HDR10/HLG)** | ❌ 不依赖 | ✅ 能（计算+编码在原生，ExoPlayer 播） |
| **在 Flutter 界面上"预览" Ultra HDR 画面** | ✅ 依赖 | ⚠️ 受限，需原生 HDR 承载 |

**你否掉 Flutter 的"HDR 显示"限制，只影响"预览"这一项；不影响你核心的"产出 HDR 文件"。**

## 二、依据（本机 Flutter 3.44.2 源码实证）

- Flutter 的 `dart:ui` 里 **`DisplayFeature`**（`window.dart` / `hooks.dart`）语义是**屏幕挖孔/铰链/折叠**，与 HDR **无关**。
- `dart:ui` 无任何 **gain-map / HLG/PQ 解码 / HDR 色彩空间合成**的渲染 API。
- 新版 Flutter 的"HDR 支持"本质是 **表面级 HDR 适应**（标记窗口请求 HDR 能力、让系统对普通内容做 tone map），**并非 Ultra HDR 增益图的合成**。

> 结论：Flutter 渲染器**读不懂 Ultra HDR 的增益图**，若靠 Flutter 画 Ultra HDR `Bitmap`，得到的是**丢掉增益图后的 SDR 底图**，不是 HDR 画面。

## 三、为什么"产出 HDR 文件"仍可行（关键）

**HDR 转换 = 像素计算 + 合规容器编码，不是"渲染 HDR"**。这两者都是**原生层**的事：

1. **gain-map 计算**：纯数学（`computeGainMap` 等），Kotlin 原生做（可复用 Electron 后端数学）。
2. **Ultra HDR JPEG 容器**：Android 官方 `android.graphics.ultrahdr.UltraHDRImage`（API 35+）`addGainmap` + `writeToFile`，原生做。
3. **HDR 视频**：官方 `UltraHDRToHDRVideo` 栈（`ImageWriter/HardwareBuffer → MediaCodec HEVC → MediaMuxer`，API 34+），原生做。

Flutter 只担任 **UI / 取图 / 文件保存 / 进度显示** —— 这些不需要 HDR 渲染。

## 四、"预览 HDR 画面"的三种兜底（在 Flutter 工程内仍可看 HDR）

| 方案 | 做法 | 是否 HDR 正确 |
|---|---|---|
| ① 原生 HDR Activity/View 承载 | 用 MethodChannel 打开一个**原生 HDR Activity**（`COLOR_MODE_HDR`）把 Ultra HDR 显示出来 | ✅ 正确 HDR |
| ② ExoPlayer/Media3 播 HDR 视频 | 产出 HDR10 视频后用 ExoPlayer 播（官方栈已验证） | ✅ 正确 HDR |
| ③ Flutter Widget 内"预览" | 直接画 `Bitmap` | ❌ 只是 SDR 降级预览 |

要"正确 HDR 预览"就选 ①/②；若可接受"文件正确 + 预览用降级 SDR"，则纯 Flutter 也够用。

## 五、选型对照（Electron 无法上安卓，两条现实路线）

| | 路线 A：Flutter `hdr_convert` 加原生桥（推荐） | 路线 B：另起原生 Android(Kotlin/Compose) |
|---|---|---|
| 复用现有 | UI/壳/多平台 | 少（全重写 UI） |
| 产出 HDR | ✅ 原生桥 → 官方 API | ✅ 直接官方 API |
| HDR 预览 | 原生 HDR Activity/ExoPlayer 兜底 | ✅ 原生 HDR 窗口直接支持 |
| 增量 | 中等（一个原生桥 + Kotlin 数学迁移） | 大 |

**若你的"预览 HDR 是硬需求"且想一步到位 → 路线 B 原生 Android（Compose）最顺**；
**若接受"产出正确 + 预览用 ExoPlayer/原生 Activity 承载" → 路线 A Flutter 也可，且最省。**

---

## 推荐

- 你的核心诉求＝**产出合规 Ultra HDR / HDR 视频** → **Flutter 可行**（推翻"不支持 HDR 就用不了"）。
- 但你明确表达了 HDR 显示顾虑 → 若**预览 HDR（而非仅输出文件）是你验收硬标准**，选 **原生 Android(Compose)**，用官方 HDR 窗口 + `UltraHDRImage`/`UltraHDRToHDRVideo` 一步到位。

---

## 附录：两个 Flutter HDR 播放插件实测对比（video_player_hdr vs flutter_tv_media3）

> 已放行网络下载 pub.dev 源码核验（Apache/BSD 许可）。

### `video_player_hdr`（1.1.0）
- **本质**：官方 `video_player` 的 **fork**，核心改一点：增加 **`viewType: platformView`**，用**原生 Android 平台视图**承载视频（而非 Flutter texture），从而**能在 Flutter 界面里正确渲染 HDR 视频**。
- **Android**：复用 `video_player_android`(fork) + ExoPlayer；自带 `VideoPlayerHdrPlugin.kt` 提供 `isHdrSupported`(=`display.isHdr`)、`getSupportedHdrFormats`(`hdrCapabilities`)、`isWideColorGamutSupported`、`getVideoMetadata`(BT2020/HLG/ST2084 等)。
- **优点**：官方 `video_player` 的 drop-in 替换（换 import 即可），**HDR 视频预览痛点的直接解法**，iOS 也支持，Flutter ≥3.44.0（本机 3.44.2 满足）。
- **局限**：是**第三方 fork** 需自行跟上游；只解决"HDR 视频"，不解决 Ultra HDR **图片**预览；需原生仍承担"产 HDR 文件 + Ultra HDR 图预览"。

### `flutter_tv_media3`（0.2.0）
- **本质**：**Android TV** 向的 Media3 播放器插件（描述="Android TV Media3 player plugin…in a separate Activity"）。
- **架构**：播放器跑在**独立的原生 Activity**（`PlayerActivity` + `PlayerView`，`Theme.AppCompat.NoActionBar`），UI 用另一套 FlutterEngine 的 overlay，控制靠 `FtvMedia3PlayerController` 单例（playlist/EPG/AFR/IP 控制/D-pad）。
- **HDR 实测**：插件**自己没有**设置 `COLOR_MODE_HDR` 窗口色模式，只在**元数据层**识别 `isHdr`/`hdrStaticInfo`；HDR 是否真渲染**取决于宿主 Activity 是否声明 HDR 窗口**。即"HDR support"=**可播放(依赖宿主声明)+可识别元数据**，非开箱即 HDR 窗口。
- **优点**：Media3 成熟播放器、AFR 帧率切换、强大播放器功能；面向 TV 场景完整。
- **局限**：**面向 Android TV、D-pad/EPG/playlist/IP 控制**，非手机单视频预览场景；"自定义 UI 需改插件源码"；不解决 Ultra HDR 图/产 HDR 文件。

### 对你的项目怎么选
| 场景 | 选 |
|---|---|
| 只是**在手机 App 里预览/播放 HDR10-HLG 视频**（Flutter 内嵌） | `video_player_hdr` 更贴合（platformView 原生 HDR 表面，简单直接） |
| 做 **TV/大屏播放器**（playlist/AFR/EPG/IP 控制），或要 Media3 全套 | `flutter_tv_media3` |
| 要 Ultra HDR **图片**预览 / **产出** HDR 文件 | 两者都不管 → 仍需原生 Kotlin + 官方 API（桥/插件） |

> 结论：`video_player_hdr` 是补"A：Flutter 内嵌 HDR 视频预览"的最佳现成组件；`flutter_tv_media3` 是**另一类需求**（TV 播放器）。两者都不替代"原生官方 API 产出/图预览"。
