# HDR 预览台（hdr_preview）

模仿 [webmproject/hdr-explorer](https://github.com/webmproject/hdr-explorer) 的**独立单文件演示页**，
用于**实时预览「传递函数（EOTF/OETF）」与「色域（Gamut）」对画面效果的影响**。

- 零依赖、无任何网络请求，**双击 `index.html` 即可在浏览器打开**（建议 Chrome/Edge）。
- 传递函数与色度学公式取自上游 `app/color_helpers/color_functions.ts`（Apache-2.0）：
  PQ（SMPTE ST 2084，峰值 10000 nits）、HLG（ARIB STD-B67 / BT.2100，峰值 1000 nits）、
  sRGB / Rec.709、Gamma 2.2、Gamma 2.8、线性（SDR 类按 203 nits 峰值约定）。

## 打开方式

```
双击 hdr_preview/index.html
```

> 📄 **渲染链路文档**：两条预览链路（场景 CPU 链 / 媒体 WebGL2 链）的完整数据流、
> 参数映射与上游对应关系见 [RENDERING.md](RENDERING.md)。

## 功能

| 模块 | 说明 |
| --- | --- |
| 媒体导入 | 点「📁 导入视频/图片」或拖放文件（MP4/MOV/AVIF/WebM/MKV/PNG/JPG），**自动读取容器色彩元数据**并填入传递函数/色域下拉（同上游「自动检测」行为） |
| 媒体元数据面板 | 显示解析结果：CICP（colr box / Matroska Colour）、Mastering display（mdcv）、MaxCLL/MaxFALL（clli）、以及**首视频样本内的动态元数据**：HDR10+（SMPTE 2094-40：targeted 峰值、maxscl、maxrgb 分布）与 2094-50（AGTM：版本、基线 headroom、mix、增益空间） |
| 媒体双预览 | 左＝原生参考（浏览器自带色彩管理/色调映射）；右＝**WebGL2 渲染**（模仿上游 BaseWebgl2Renderer + HdrRenderer 写法）：`createImageBitmap(video, {colorSpaceConversion:'none'})` 取未做显示转换的帧 → RGBA16F 纹理 → GLSL 内 按所选传递函数解码→HLG OOTF（γ 随显示峰值自适应）→ 色域转换 → headroom 缩放 → 显示 OETF——修改任何 HDR 参数即时生效；`configureHighDynamicRange({mode:'extended'})` + `drawingBufferStorage(RGBA16F)` 探测 HDR 画布（Chrome/Edge），不可用时自动回退 2D-CPU 链；视频支持播放/暂停/进度/循环/帧宽调节（面板徽标显示渲染路径与 HDR 画布状态） |
| 主预览 | 程序化 HDR 场景（日落高光 + 测试色卡），实时走完整链路：内容编码 → 解码 → HLG OOTF → 色域转换 → 色调映射 → 显示编码；底部有 1~10000 nits 灰阶标尺，鼠标悬停可读单像素数值 |
| EOTF 曲线 | 各传递函数「码值 → 亮度(nits)」对数图，当前传递函数高亮并标注 100/203/1000 nits 锚点 |
| OETF 曲线 | 各传递函数「亮度(nits) → 码值」对数图，可读 10bit 码值 |
| CIE 1931 xy | 光谱轨迹 + 内容色域三角形 + 显示色域三角形 + 内容颜色采样点（圆点显示实际显示效果），非 D65 白点以彩色圈标出 |
| 控制项 | 内容：传递函数 9 种（PQ / HLG / sRGB / Rec.709 / γ2.2 / γ2.8 / Rec.2020 10bit / Rec.2020 12bit / 线性，选项与上游一致）、内容色域 11 种（BT.709 / BT.470-6 M / BT.470-6 BG / BT.601 / SMPTE 240M / Generic film / BT.2020 / XYZ / RP 431-2 / P3 / CICP 22，与上游一致）、曝光 EV、预设；渲染：显示峰值 / 色调映射（无、Reinhard、ACES）/ 显示色域（同样 11 种）/ 显示传递 / 超色域处理（裁剪、软化）/ 超范围·超色域标记（粉/青）/ HLG OOTF 开关 |

## 元数据解析能力与限制

- **容器层**：MP4/MOV/AVIF 的 `colr`（nclx→CICP）、`mdcv`、`clli`；WebM/MKV 的 Matroska `Colour`（Primaries / TransferCharacteristics / MatrixCoefficients / Range / MaxCLL / MaxFALL）。
- **码流层（首视频样本）**：H.264/H.265 SEI（payloadType 144/137）与 AV1 OBU（HDR_CLL / HDR_MDCV / ITUT_T35）中的 HDR10+ 与 2094-50 元数据；对 `has_size_field=0` 等非标准 OBU 布局有 T.35 特征盲扫兜底。
- **与上游的差异**：上游逐帧解析全部动态元数据并以 WebGL2 渲染 HDR 帧缓冲；本页只解析首样本并输出摘要，渲染走 2D canvas CPU 链，面向教学演示。
- **帧值说明**：浏览器只会把解码帧转换到显示色彩空间后才交给页面（SDR 屏即 sRGB），因此对 HDR 素材「重渲染」是对浏览器转换结果的近似重解码（对 SDR 素材则完全精确）——这正是「把同一素材按不同元数据参数解读」的演示语义。

## 与上游 hdr-explorer 的差异（当前简化）

- 上游支持 SMPTE ST 2094-50 元数据生成、3D LUT 渲染、HDR 真窗口输出（多面板 + headroom 原生检测）等；
  本页媒体渲染采用上游同款 WebGL2 渲染器结构（`kColorFunctionGlsl` 逐字搬运 + `BaseWebgl2Renderer` 模式），
  但为单文件教学工具：单面板、无原生 headroom 检测（`navigator.hdr` 未接入）、动态元数据仅首样本摘要。
- 场景预览与曲线图为 2D canvas CPU 实现；媒体路径 GPU/CPU 双轨自动切换。

## 后续路线

- 全样本动态元数据解析（对应上游逐帧解析）与时间轴关联
- 接入 `navigator.hdr` 原生 headroom 检测与多面板布局（对应上游 Headroom fieldset）
- 2094-50 曲线编辑（对应上游 Curves 面板）与 3D LUT 渲染
- 直方图/CDF 统计（对应上游 Stats 面板）
- 接入本仓库 Electron 项目（hdr_electron）作为内嵌页面