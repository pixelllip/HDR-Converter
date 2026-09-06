# 更新说明（对比 0.3.2 发行版）

> **对比基线**：`git tag 0.3.2`（提交 `7c20fd5`，2026-09-03）
> **当前状态**：HEAD `596cbaa`（2026-09-06）+ 工作区未提交改动（均已包含在本说明内）
> **规模**：0.3.2 → HEAD 共 24 个提交（30 文件，+7861 / −337）；工作区另有 34 个文件改动（+249 / −1136，主要是视频链路清理）

---

## 一、HDR 预览能力（本次最大变化）

1. **新增独立 HDR 预览台 `hdr_preview/`**：模仿 webmproject/hdr-explorer 的单文件演示页，零依赖，双击可在 Chrome/Edge 打开，实时预览「传递函数（EOTF/OETF）」与「色域」对画面的影响；渲染链路数据流见 `hdr_preview/RENDERING.md`。
2. **视频页转换前预览升级为实时视频预览**（`views/hdr_preview_media.js`，替换旧「首帧图片 /preview 往返」链路）：
   - 场景 CPU 链 + 媒体 WebGL2 链两条渲染路径，内容区参数即时生效；
   - 启用 Chromium **CanvasHDR**（恢复 HDR 画布 API，`--enable-features=CanvasHDR`）；
   - 显示端**软膝色调映射**（tanh 软滚降替代硬钳到显示峰值，消除大片死白/过曝）；
   - 新增「目标渲染亮度」参数（显示峰值语义，拖动即时改变整体亮度）；
   - 预览输出端定标语义与上游 hdr-explorer 同源（display-referred extended-sRGB 直出）；
   - 移除与源视频 pane 重合的原生参考画布，预览区只保留内容区重渲染；
   - 移除语义重复的「曝光 EV」控件。
3. **导入素材自动读取色彩元数据**（CICP / colr box / Matroska Colour / mdcv / clli）填入传递函数/色域下拉，与 hdr-explorer「自动检测」行为一致；预览=导出同参数模型（WYSIWYG）。

## 二、输入信号解读参数化（P1）

- Rust 后端转换管线参数化**输入传递函数 / 输入色域**：
  - `auto`：按视频元数据解读（PQ/HLG 照用）；SDR 内容默认按 **HLG** 解读（避免 PQ 解读 SDR 素材发暗/过曝）；
  - 输入色域 709 / 2020 / P3 等，驱动后端线性化与 zscale `pin=`；
- 端到端验证：`tests/verify_input_signal.js`（transform 链路三种输入解读对照）。

## 三、导出参数重构

- 「目标渲染亮度」与「内容峰值亮度」**合并为一个「峰值亮度」**（内容端 max-cll / npl 语义）；
- 输出传递函数/色域**拓宽并与输入合并为一组控件**（传递函数 auto / PQ / HLG 等，色域 auto / bt2020 / p3 等，输出跟随「内容信号」合并语义）；
- **移除视频侧伽马 / RGB 通道调节**（无调整需求，固定 gamma=1.0、RGB=1:1:1），视频参数集简化。

## 四、Eclipsa（ST 2094-50）动态预览与预分析

- 新增 `hdrconv analyze-eclipsa`：导出前对素材做**逐窗预分析**（scene 镜头切 / uniform 均分、MaxCLL/Hbaseline），为预览端动态 2094-50 渲染预生成窗口表；
- 前端**动态 2094-50 预览**（逐窗 AGTM 参考白配方渲染，拖动时间轴按窗应用增益）；
- 源传函自动探测（PQ/HLG/SDR），修复 SDR 源假峰值；**SDR 源显示模拟 HDR 峰值**（SDR 白点 → 峰值亮度语义化显示）；
- HLG 基带分析按 HLG EOTF + BT.2100 OOTF 换算亮度（0.75 码值 ≈ 203 尼特漫白锚点，公式与 hdr_preview 常数一致，替代旧 PQ EOTF 全 0 失效问题）。

## 五、视频链路清理（工作区未提交改动，随本版一并交付）

- **彻底移除视频侧「逐帧增益图（Ultra HDR 式）」链路**：
  - 前端「转换方式」选择器（早已隐藏）与相关文案清理；
  - JS：`video_converter.js` 删除 `transformMode` 选项与 gainmap 分支，固定 `mode:'transform'`；
  - Rust：`/video-frame` 删除 mode=gainmap 分支、`hdrconv video --mode frames`、`reconstruct_linear_hdr_frame`、gpu.rs gainmap16 FFI/FrameMode；视频帧重建 GPU 泵仅保留 transform16；
  - CUDA：删除 `kFrameGainMap16` / `kFrameGainMap16Masked` 内核、对应 FFI/JNI 导出与 mask 缓冲，**已重新编译 `hdr_gpu_ffi.dll`**（transform16 等其它内核不受影响）；
  - 测试：删除 gainmap 专属回归（软阈值 / flicker 抑制），视频链路验证脚本统一为单层色调映射语义；
- 视频链路固定**单一「逐帧单层色调映射」（transform）**：线性化 → ×RGB ×曝光（=峰值/白点） → 伽马 → Rec.2020/PQ；产物以 HDR10 元数据（mdcv/clli）承载，不内嵌 ICC；
- 修正「图片 ICC 增益式」等表述（视频链路不含 ICC），README / MEMORY / Rust README / 集成计划 / 设计稿同步对齐；
- 清理 `backend/kotlin/` 空壳目录与 Kotlin 构建诊断日志（Kotlin 源码仍留档于 `archive/kotlin-backend/`）。

## 六、已知限制与说明

- 视频逐帧重建的 GPU 加速（`FramePump` / transform16）**仅 `hdrconv video` CLI 链路接入**；Electron 界面视频转换（HTTP `/video-frame`）仍走 CPU（待接线）；
- Kotlin JVM 后端仅存于 `archive/kotlin-backend/`（供 Kotlin 时代对照测试与旧产物复现），运行不再使用；
- 图片侧 Ultra HDR JPEG（增益图双 JPEG）能力不受视频链路清理影响，完整保留。

---

## 附录：0.3.2 → 当前 主要提交清单（24 commits）

| 主题 | 提交 |
|---|---|
| Eclipsa 预分析（SDR 峰值模拟 / 源传函 / 字段输出） | `596cbaa` `32524ad` `bb88a74` |
| 动态 2094-50 预览（analyze-eclipsa + 逐窗 AGTM 渲染） | `f57032e` `a39eb9d` |
| 输出参数重构（TF/色域合并、移除视频伽马/RGB、峰值合并） | `8bbe546` `54b3847` |
| 输入信号解读参数化（Rust + 端到端验证） | `94bf1b3` `5916023` `b64a308` |
| 视频实时预览移植与修正（CanvasHDR / 软膝 / 目标渲染亮度） | `4abf5d1` `7adc009` `686e5f0` `8f327f5` `1ebb6cf` `d799256` `a09f087` `ac27dc5` `159fbf3` |
| HLG 断言/单测修正（与 hdr_preview 常数一致） | `fa97f79` |
| HDR 预览台（hdr_preview）与集成计划 | `8e8cbd9` `0ea4543` `ac00812` |
| 工作区快照 | `09b3a44` |