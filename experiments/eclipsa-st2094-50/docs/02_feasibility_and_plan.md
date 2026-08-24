# 引入“Eclipsa video（基于 SMPTE ST 2094-50）”可行性分析与集成方案

> 核心交付文档。阅读顺序：`README.md` → `01_st2094-50_primer.md` → 本文。
> 本文结论基于 `../reference/SOURCES.md` 记的 PCD2（2026-02-23）草案、
> 以及对本项目现有代码/工具链的实测（2026-08）。

## 0. 目标再定义

“把 Eclipsa video(Eclipsa 视频标准)引入本项目”在本实验里落实为：

- **生成侧（本实验主目标）**：让本项目转换出的 HDR 视频（当前为 HDR10 静态元数据）
  额外携带 **ST 2094-50 Application #5 动态元数据**——即每个时间窗一段
  `smpte_st_2094_50_application_info`（Annex C 二进制），通过 **ITU-T T.35 标识**
  （country `0xB5` / terminal provider `0x0090` / oriented code `0x0001`）承载。
- **验证侧（可选）**：能自写解析/校验工具验证携带正确性，并评估播放端识别情况。
- 现阶段只写方案，不改产品代码（本次交付范围，用户已确认）。

### 0.1 “支持 HDR10+ 就等于支持 Eclipsa Video 吗？”——澄清

**答案：不成立，不能划等号。**（官方材料已在 2026-05 公开，见 `../reference/SOURCES.md` §1b。）

| 维度 | HDR10+ | Eclipsa Video |
|---|---|---|
| 底层应用规范 | **SMPTE ST 2094-40**（Application #4） | **SMPTE ST 2094-50**（Application #5, Broadcast） |
| T.35 标识 | HDR10+ 专用标识（与 2094-50 不同） | country `0xB5` / terminal provider `0x0090` / oriented code `0x0001` |
| 动态元数据语法 | HDR10+ 窗口/JSON 体系 | **Annex C** 参考白 + HATM 增益曲线 |
| 识别/渲染 | 解析 2094-40 载荷的播放器 | 需实现 2094-50 语义的播放器 |

- 一个只“支持 HDR10+”的设备**不会自动**渲染 Eclipsa Video 的动态元数据——它只会把
  ST 2094-50 的 SEI 当作未知数据忽略（静态 HDR10 画面仍正常）。
- 官方原文用的是 **“Devices certified for **both** standards may utilize ‘Eclipsa Video powered by HDR10+’”**——
  “双标准认证”字样本身就说明要**分别认证**；官网还提到独立的 **licensing / testing** 程序。
- “无缝整合”的真实含义：同一管理联盟（HDR10+ Technologies）、可叠加的品牌背书、共存于同一 HDR 生态，
  **而非解码等价**。
- 对本项目：输出“Eclipsa Video 文件”= **现有静态 HDR10 + 新增 ST 2094-50（Annex C）动态元数据**，
  二者互补共存；不能拿生成 HDR10+ 的工具（x265 `--dhdr10-info` 等）来偷懒（载荷不同，见 §2）。

## 1. 与现有管线的契合度（为什么这事“值得做且不难”）

### 1.1 直接对齐的既有事实

| 项目现状 | 对 ST 2094-50 的意义 |
|---|---|
| 默认白点 / 参考白 **203 尼特**（BT.2408，README） | 正是草案默认 `Lwhite=203` |
| 输出 BT.2020/PQ（HEVC main10） | 草案增益应用色域默认首选 BT.2020（C.3.8） |
| 视频链路 2 逐帧重建出**线性 HDR**（float64，增益图式高光扩展） | **Hbaseline/窗口峰值统计的现成数值源** |
| 图片 Ultra HDR 增益图（ISO 21496-1 / ICC 增益曲线思想） | 与 6.3~6.5 的增益曲线/分量混合**数学同构** |
| `mp4_hdr.js` 已有“注入盒 + 向上传播祖先盒尺寸”基建 | 容器/码流注入的工程范式可复用 |
| `video_converter.js` 已统一 x265 参数串、编码器可选、remux | 后期加“附加元数据”只动收尾环节 |

### 1.2 关键简化点（草案自带红利）

草案 **C.3.8 参考白预设配方**让编码端**不必拟合任意曲线**：
只需给出每窗 `Hbaseline = log2(窗口峰值亮 / 参考白)`，其余（色域=2020、
2 个 alternate、8 控制点增益曲线与斜率、膝点）全部由公式自动合成。
而这正对本项目“已从逐帧/逐窗 HDR 统计能给出峰值”的能力。

## 2. 工具链实测（2026-08，本机项目自带 ffmpeg）

| 项 | 实测结果 | 结论 |
|---|---|---|
| ffmpeg 版本 | `9.0-essentials_build-www.gyan.dev`（含 libx265/HEVC/AV1/nvenc） | 基线正常 |
| x265 动态元数据参数 | 支持 `--dhdr10-info`（“Unable to open tone-map file”证明选项被识别）；`hdr10-plus-opt` 不存在；`hdr10-opt` 存在 | **x265 端有动态元数据通道，但** |
| 载荷格式 | x265 `--dhdr10-info` 注入的是 **HDR10+/ST 2094-40** 载荷（其 JSON schema），**不是** Application #5 的 Annex C | **不能直接复用 → 需自研 Annex C 编码 + SEI 装配** |
| Windows 路径坑 | `-x265-params "dhdr10-info=C:\…"` 的 `:` 被分隔符截断（`Unknown option: …empty.json:colorprim`） | 即便用 x265 也需 **basename + cwd** 技巧 |
| 位流滤镜 | `hevc_metadata` 存在（aud/元数据 SEI 操作先例） | 有“纯码流复制+注入”现成模式，但载荷同样非 2094-50 |
| **结构性能力差** | 现成工具链无任何一项输出 Application #5 载荷 | **必须自研编码器**（位级已由 Annex C 定死，工作量可控） |

> 一句话结论（2026-08 更新）：**FFmpeg 9.0 已原生支持 SMPTE 2094-50（解析 / 序列化 / 透传），
> 大幅降低了“编码器必须完全自研”的风险——自研部分可收窄为“数值生成 + 注入”，
> 位级正确性可直接借 FFmpeg 9.0 的参考实现做校验/序列化。**

### 2.5 FFmpeg 9.0 原生支持（重大进展，2026-08 源码核实）

本项目自带的就是 **ffmpeg 9.0 essentials**；9.0 官方变更日志第 12 条即
**“SMPTE 2094-50 metadata support and passthrough”**。按 n9.0 源码核实其实现范围：

| 层 | 位置 | 能力 |
|---|---|---|
| libavutil | `hdr_dynamic_metadata.h/.c` | 新增 `AVDynamicHDRSmpte2094App5`（字段与 **Annex C.2 逐条对应**，≤4 alternate、≤32 控制点）；`…_from_t35()` 解析 / `…_to_t35()` 序列化（不含 T.35 头）；`AV_FRAME_DATA_DYNAMIC_HDR_SMPTE_2094_APP5` side data |
| libavcodec | `itut35.c` / `decode.c` | **HEVC/AVC 的 T.35 SEI 解析**：provider=SMPTE(0x0090)+oriented=1 → App5 side data；packet↔frame side data 映射 |
| libavformat | `matroskaenc.c` / `matroskadec.c` | **MKV 写/读**：自动加 T.35 头（`B5 00 90 00 01`），App5 写入 BlockAdditional / 读回 side data |
| libavcodec | `libaomenc.c add_hdr_smpte2094_app5()` | **AV1(libaom) 编码端直接写 2094-50 OBU**（帧带 side data 时） |
| fftools | `ffprobe.c` | `-show_frames` 打印全部 App5 字段（可当**参照解析器**） |
| — | `movenc.c` / `mov.c` | **MP4 容器盒不支持**（符合草案容器映射未定） |
| — | 滤镜 | **无生成滤镜**（libplacebo 只有 2094-40 色调映射）；CLI 无入口直接挂 frame side data |

要点：
1. **T.35 常量已确认**：`COUNTRY_US=0xB5`、`PROVIDER_SMPTE=0x0090`、oriented `0x0001`，与草案一致。
2. **“支持+透传” ≠ “自动生成”**：9.0 能解析/序列化/携带/打印/AV1 编码，但 **CLI 没有把
   frame side data 注入管线的入口**——所以“算出数值并放进流里”仍需我们做（JS 编码器或小 C 助手）。
   但现在有了**参考实现**：可作为校验基准，也可用 `to_t35` 直接序列化。
3. 对产线最实用的两条现成通道：
   - **HEVC 走带内 SEI**（我们的注入）+ **ffprobe 校验**（参照解析器）；
   - **AV1 走 libaom-av1 编码**（side data 喂给编码器即写 OBU）+ **MKV 容器原生携带**。
4. 本地 gyan **essentials** 构建含 libavcodec/libavformat/libavutil 与 `--enable-libaom`，
   但 **P0 实证（2026-08）**：其 ffprobe **不导出 2094-50 side data**（HEVC 前缀/后缀、AV1 OBU
   注入均不导出；x265 自带的 Unregistered SEI 却能导出）→ **不要在产品里依赖本地构建的
   2094-50 解析**；校验用 trace_headers（结构层）+ 本实验自写解码器（`poc/st2094_50.js` 已具备），
   或换支持 2094-50 的 ffmpeg 构建。

## 3. 目标元数据生成流程（生成侧落地方案）

### 3.1 数据从哪来（复用现有计算，不新增分析引擎）

```
逐窗(Scene/Window)统计 —— 在现有链路内顺带采集：
  每窗 MaxCLL            → Hbaseline = log2(MaxCLL / Lwhite)
  每窗 MaxFALL(可选)      → 用于 alternate/曲线倾向参考
  每窗起始帧号/时长        → TimeInterval(Start, Duration)
  ProcessingWindow       → 固定全图(草案约束: 0,0 → 宽,高)
Lwhite                   → 用户峰值亮度设置的“参考白”一侧（默认 203）
```

- 窗口划分建议：**GOP 对齐**或**场景切换检测**（可用现有逐帧重建期间的亮度差）；
  草案对窗口时间粒度无限制，先从“整段视频 1 窗（全静态）”或“按场景 N 窗”开始。
- 初版可退化成“整段一个 TimeInterval + 一个 Hbaseline（用全局峰值）”——
  已是合法的 Application #5 元数据，之后再加场景级细分。

### 3.2 编码与装配（P0 核心）

1. **Annex C 编码器**（新模块，如 `st2094_50.js`）：按 C.2/C.3 把
   `{Lwhite, TimeInterval, Hbaseline, 配方/自定 alternate}` 编为
   `smpte_st_2094_50_application_info` 二进制（大端 u16；≥1 字节对齐）。
   最小用例对标：仅参考白（无 HATM）→ 载荷 `00 00`；参考白+HATM 配方(Hbaseline=2)
   → `00 40 4E20 80` 形态（见 Primer §5.6 核对）。
2. **T.35 前缀**：`ITU-T T.35` 头 `0xB5 0x00 0x90 0x00 0x01`（国家码/终端提供商标识，
   顺序以 T.35 现行规范与正式版核对）。
3. **码流 SEI 装配**：包成 HEVC **`user_data_registered_itu_t_t35`** SEI（NAL type 39 前缀 /
   40 后缀任选，建议可配置），作为 `Prefix_SEI`/`Suffix_SEI` 插入到每个窗口首帧的
   access unit（在 `-c:v copy` remux 前对 HEVC ES 注入）。
4. **注入时机（两种，取其一）**：
   - **A1 编码后注入 ES**：从 hvc1 track/文件解出 HEVC ES → 注入 SEI → remux 回 mp4。
     拥挤点在于要维护 access unit 边界；SEI 在 mdat 内、不动 moov → **无需祖先盒传播**。
   - **A2 remux 时用自定义 bsf**：把编码器做成 ffmpeg bitstream filter（或先在
     `video_converter.js` 里用“解码 ES→注入→重新 mux”的 Node 侧等价实现，避免编译 ffmpeg）。
     初版建议 **A1 的 Node 侧实现**（与 `mp4_hdr.js` 同一工程风格、零编译）。
5. **（新增 2026-08）借 FFmpeg 9.0 参考实现**，三选一：
   - 纯 JS 自编码 → 用 `ffprobe -show_frames` 做**参照解析器**回读校验；
   - 或写 ~100 行 C 小助手链接 FFmpeg 9.0 的 `av_dynamic_hdr_smpte2094_app5_to_t35()`
     （我们只算数值、输出 T.35 载荷字节），位级正确性交给参考实现；
   - AV1 / MKV 通道：side data 就绪后由 libaom / matroska **原生携带**（无需我们拼字节）。

### 3.3 容器级携带（后续阶段 B）与“容器优先”条款

草案 7.3 规定：码流与容器都支持 T.35 时**容器优先**。但 PCD2 未给容器盒映射细则
（ISO BMFF 里如何在采样条目/`udta` 表达 Application #5 仍在委员会工作中）。
因此：
- 阶段 A 先做**码流携带**（HEVC SEI，无歧义、可 mux 进 mp4/mkv）；
- 阶段 B 待容器映射明确后再补容器盒（届时复用 `mp4_hdr.js` 的注入基建）。
- 补充（2026-08）：**MKV 容器侧已由 FFmpeg 9.0 原生支持读写**（见 §2.5）；
  目前空白只在 **MP4 盒**这一处。
- 验收时明确说明“当前按码流携带实现；容器优先为后续项”。

## 4. 集成落点（在本项目代码中的改动蓝图，供将来实现）

| 模块 | 改动 |
|---|---|
| `video_converter.js` | 收尾阶段：从后端/统计得到每窗 `{T0, dur, MaxCLL}` → 调 `st2094_50.js` 编码 → 注入 ES → remux（新增 1 个导出如 `attachSt2094_50()`） |
| `mp4_hdr.js` | （阶段 B）容器盒注入复用其定位/尺寸传播逻辑 |
| 后端 Kotlin（可选） | 逐窗统计（MaxCLL/MaxFALL）在重建循环内顺带返回，减少二次扫描 |
| `main.js` / IPC | 新增“是否附加动态元数据”设置项与进度反馈（后续） |
| 设置 UI | 一个开关 + 参考白数值（默认 203，复用现有白点） |
| 测试 | 新增 `tests/verify_st2094_50.js`（见 §6） |

## 5. 三种方案取舍

| 方案 | 做法 | 优点 | 缺点 | 建议 |
|---|---|---|---|---|
| **A（推荐，先做）** | JS 数值生成 + Annex C 编码 + HEVC SEI 后处理注入；编码正确性借 FFmpeg 9.0（`to_t35`/ffprobe）校验 | 产物 = 真正的 Application #5；与现有 x265/HDR10 链路最贴合；参考实现兜底 | 需自写编码与 NAL 装配 | 与“注入盒”哲学一致；工程量 ≈ mp4_hdr.js（风险已大幅下降） |
| **A′（可选增强）** | 小 C 助手调 FFmpeg 9.0 `av_dynamic_hdr_smpte2094_app5_to_t35()` 做位级序列化 | 位级正确性 100% 交给参考实现 | 需编译一个 mini 助手 + 进程调用 | 想彻底免除手写编码器时启用 |
| **B（AV1 通道，后续）** | 帧 side data → `libaom-av1` 编码（FFmpeg 9.0 原生写 2094-50 OBU）；或 MKV 容器原生携带 | 与未来 Chrome(Chromium+AV1) 路线契合；MKV 免自拼字节 | 需先把 side data 送进编码器；不是现有默认 HEVC 链路 | 阶段 A 验证后再评估 |
| **C（否/否）** | 复用 x265 `--dhdr10-info` / 其他 HDR10+ 工具 | 省开发 | **载荷是 2094-40 不是 2094-50**；Windows 路径坑；与 Eclipsa 目标不符 | **否** |
| **D** | 容器级携带（MP4/TS 盒）+ 反向解析/播放端 | 满足“容器优先”；可反读校验 | MP4 盒映射草案未定、FFmpeg 9.0 也未支持 | 阶段 B/C 后续再做 |

## 6. 验证方案（对齐本项目“每个验证一个脚本”惯例）

1. **编码器自检（双参照）**：自编 → ① 自写 parser 回读；②（更强）把载荷包成带
   `B5 00 90 00 01` 头的 HEVC SEI → 喂 **`ffprobe -show_frames`**，与 FFmpeg 9.0 的
   `from_t35` 解析结果逐字段比对（重点覆盖：默认/自定义参考白、Nalt=0、Nalt=2+配方、
   PCHIP 与 θ 两种斜率）。
2. **码流验证**：对合成 SDR→HDR 输出，`ffprobe`/自制 NAL 扫描断言
   `user_data_registered_itu_t_t35` SEI 存在、载荷前缀 `B5 00 90 00 01`、NAL 类型 39/40
   落在对应 access unit。
2b. **本地构建实测（2026-08 已完成）**：gyan essentials 9.0 的 ffprobe **不打印** App5 字段
   （P0 已实证）；验证脚本做成**能力自适应**：不支持的构建自动跳过语义断言并保持 exit 0，
   换支持 2094-50 的构建时自动恢复全量比对。结构层校验用 trace_headers（P0 已通过）。
2c. **原生通道验证（可选）**：MKV 写入→读回 round-trip；AV1(libaom) 编码→ffprobe 确认 OBU。
3. **容器无损**：注入后 `ffmpeg -c copy` remux mp4；对比注入前后
   `mdat` 偏移/`stco` 不变（SEI 在 mdat 内）或按需要调整——复用 `mp4_hdr.js` 已解决的机制。
4. **语义抽查**：取窗内峰值帧，手动核对 `Hbaseline = log2(MaxCLL/参考白)` 与编码值一致。
5. **播放端观测（记录性，2026-08 更新）**：本项目 Electron 33 的内置 Chromium 内核（≈2024）
   **早于** ST 2094-50 支持（Chrome 官方 2026-05 公告：后续版本才支持 finalized 的 2094-50）——
   因此**当前预览画面不会因动态元数据变化**。但这是“内核版本差异”而非“永远读不到”：
   升级到支持 2094-50 的 Chromium/Electron 后即可识别 → 现在生成 Eclipsa Video 文件具有前瞻价值。
   回归门槛仍以自写验证脚本为准。

## 7. 阶段划分与工作量（估计）

| 阶段 | 内容 | 相对工作量 |
|---|---|---|
| P0 | Annex C 编码器 + 自写 parser + **ffprobe/trace_headers** 参照校验（含 C.3.8 配方） | **已完成**（2026-08，`poc/`，24/24 通过） |
| P1 | HEVC SEI 装配 + 后处理注入 + remux + 验证 | **已完成**（2026-08，`poc/inject_st2094_50_video.js`，真实视频 3 窗动态注入 200/200 通过） |
| P2 | 逐窗统计接入（复用逐帧重建/`/video-frame`） + 窗口划分 | M |
| P3 | 容器级携带（草案明确后） + 反向解析/预览决策 | L（依赖规范进度） |

## 8. 风险与未决项

1. **规范未定稿**：PCD2 评论期已过（2026-03-16），正式版或新 PCD 可能改动
   （位计数、标识码）。→ 实施前拉取最新仓库核对；位级“以正式版为准”。
2. **参考白定点公式**：PCD2 的 HTML 里 `hdr_reference_white` 公式疑似排版错乱
   （`×15` vs `÷15` 语义）→ 以正式版/PDF 核对后再定实现。
3. **容器映射空白**：7.3 未给 MP4/TS 盒映射细则 → 阶段 A 只做码流携带并明示。
4. **专利**：SMPTE `PATENTS.md` 声明可能存在相关专利 → 商业发布前专利排查。
5. **消费端内核版本**：Electron 33 的 Chromium 不含 ST 2094-50（Chrome 稍后版本才支持）→
   当前预览不识别、验收靠自写验证；升级内核后自动受益。不阻塞生成侧。
6. **Eclipsa Video 身份已官方确认**（开源标准、基于 ST 2094-50、由 HDR10+ Technologies 管理），
   但**它的“承载/封装细则”官方尚未公开**（是否规定容器盒/文件扩展/需经认证测试）。
   阶段 B 前通过 licensing/testing 渠道（www.eclipsamedia.org）确认，尤其“是否必须走官方法认证流程”。
7. **本地 essentials 构建完整性（2026-08 实证）**：gyan essentials 9.0 的 ffprobe **不导出**
   2094-50 side data（HEVC 前缀/后缀、AV1 OBU 注入均不，P0 已实测）→ 校验靠 **trace_headers +
   本实验自写解析器**；需要 ffprobe/预览侧能看到时，换支持 2094-50 的 ffmpeg 构建。**不影响
   “编码+注入”可行性**（P0 已验证）。
7. **许可**：把规范实现为代码前先与 SMPTE/10E 确认许可与版税（见 SOURCES 第 5 节）。

## 9. 结论

- **可行性：高**。规范完备（Annex C 位级定死）、无需拟合曲线（C.3.8 配方）、
  项目已有数值源（逐帧 HDR 统计 + 默认 203 参考白 + 增益图数学同构）。
- **不可低估的是“编码/注入自研”**：不能用现成 HDR10+ 工具（载荷不同），
  但自研量级 ≈ 现有 `mp4_hdr.js`。
- **推荐路径**：先做 P0（Annex C 编码器+自检，能证明“编得对、读得回”），
  再 P1（HEVC SEI 后处理注入）→ 拿到真实 Eclipsa 文件后核对 → 再上 P2 逐窗统计。
- **本次交付**：本文 + 规范精读 + 来源备忘；未改动产品代码（与用户约定一致）。
