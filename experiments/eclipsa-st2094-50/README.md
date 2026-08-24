# 实验：为本项目引入 “Eclipsa video”（基于 SMPTE ST 2094-50）

> 实验日期：2026-08 · 状态：**方案文档完成，未改产品代码**（与口头约定一致：先写可行性+集成方案）

## 目标

探讨把 **“Eclipsa video”**（经确认 = 基于 `https://github.com/SMPTE/st2094-50`
构建的视频标准，规范核心即 ST 2094-50 本身）引入本项目（HDR Converter Electron）：
让本应用转换出的 HDR 视频除了当前 HDR10 **静态**元数据（`mdcv/clli`）之外，
再携带 **ST 2094-50 Application #5 动态颜色体积变换元数据**，并按规范以
**ITU-T T.35 标识**（country `0xB5` / terminal provider `0x0090` / oriented code `0x0001`）
承载。

## TL;DR（结论速览）

- **“Eclipsa Video”身份已官方确认**（2026-05 新闻稿）：**基于 SMPTE ST 2094-50 的开源视频标准**，
  由 **HDR10+ Technologies LLC** 管理与认证。**但“支持 HDR10+ ≠ 支持 Eclipsa Video”**：
  前者是 ST 2094-40（Application #4），后者是 ST 2094-50（Application #5），T.35 标识与载荷语法都不同，
  官方“Devices certified for **both** standards (‘Eclipsa Video powered by HDR10+’)”的说法就表明需要**分别认证**。
- **可行性高**：草案（PCD2, 2026-02-23）的 **Annex C 二进制格式位级定死**；
  **C.3.8 “参考白预设配方”**让编码端只需给出每窗 `Hbaseline=log2(峰值/参考白)` 就能
  自动合成整段元数据——正好对应本项目已有的逐窗 HDR 统计能力和默认 **203 尼特参考白**。
- **重大进展（2026-08 核实）：FFmpeg 9.0 已原生支持 SMPTE 2094-50**（变更日志第 12 条
  “SMPTE 2094-50 metadata support and passthrough”）。源码确认：`AVDynamicHDRSmpte2094App5`
  结构体字段与 **Annex C 逐条对应**、`from_t35/to_t35` 解析/序列化、HEVC/AVC T.35 SEI 解析、
  **MKV 原生读写**、**AV1(libaom) 编码端直接写**、ffprobe 可打印全部字段；
  但 MP4 容器盒不支持、且 **CLI 没有注入 frame side data 的入口** → 我们仍需“算数值 + 注入”，
  但位级正确性可借参考实现校验。
- **主要工作不是“算”，而是“编/注入”**：HDR10+ 工具（x265 `--dhdr10-info` 等）注入的是
  2094-40 载荷，**不是** Application #5；但因为有 FFmpeg 9.0 参考实现兜底，
  自研编码器 + HEVC SEI 装配的风险已大幅下降（量级仍 ≈ `mp4_hdr.js`）。
- **推荐路径**：P0 Annex C 编码器+自签 parser 自检 → P1 HEVC SEI 后处理注入 + remux →
  P2 逐窗统计接入 → P3（规范明确后）容器级携带。
- **预期沟通项**：当前 Electron 33 的内置 Chromium **早于** ST 2094-50 支持（Chrome 官方 2026-05
  宣布后续版本才支持），所以**现在**预览不会因动态元数据变好；但升级 Chromium 内核后即受益，
  现在生成 Eclipsa Video 文件有前瞻价值。验收先靠自写验证脚本。

## 目录导航

| 文件 | 内容 |
|---|---|
| `README.md` | 本文（入口 / TL;DR / 下一步） |
| `docs/01_st2094-50_primer.md` | **规范精读笔记**：概念、数据结构、元数据集、Annex C 二进制字段表、C.3.8 配方 |
| `docs/02_feasibility_and_plan.md` | **可行性分析 + 集成方案**（核心交付）：契合度、工具链实测、生成流程、落点蓝图、方案取舍、验证、阶段与风险 |
| `reference/SOURCES.md` | 官方来源、版本时间线、许可/合规备忘（含临时查阅位置说明） |

## 需要你确认的开放问题（决定阶段 B 方向）

1. **Eclipsa Video 官方承载/认证细则**：身份已确认（基于 ST 2094-50，HDR10+ Technologies 管理），
   但其**封装/容器细则与 licensing/testing 流程**官方尚未公开。是否需在阶段 B 前通过
   www.eclipsamedia.org 核实（例如“是否必须走官方法认证”），还是先按草案 7.3 的码流携带起步？
2. **范围**：现阶段只要**生成侧**（输出带动态元数据），还是要同时做**反向解析/播放**？
3. **交付节奏**：先做 P0（编码器+自检）小步验证，还是直奔 P1（SEI 注入）？

## P1 已做（2026-08）：把真实视频转成「HDR10 + ST 2094-50 动态元数据」

`D:\video\video_sdr\bg_waifu2x_2x_2n_mp4_Eclipsa_ST2094_50.mp4`（7.73MB，4K/30fps/6.67s）
- 实测 3 窗 MaxCLL 562/527/508 尼特 → Hbaseline 1.47/1.38/1.32 档（参考白 203，C.3.8 配方）
- 200 条 `User Data Registered ITU-T T.35` SEI（每帧/窗），逐 AUD 自验一致；静态校验全绿。
- **金标准闭环**：BtbN master ffprobe（2026-08-20）→ `side_data_type: HDR Dynamic Metadata
  SMPTE2094-50`，逐帧解析 3 窗 baseline=14691/13763/13233，与注入值完全一致。
- 脚本：`poc/inject_st2094_50_video.js`，细节见 `poc/README.md` P1 节。

## 第三格式已封装进应用（2026-08）：format='eclipsa'
- 产品模块提升到项目根：`st2094_50.js`（Annex C 编解码）+ `st2094_50_inject.js`（分窗统计+注入+remux）
  + `hevc_inject.js`；已加入 `package.json` `build.files`。
- 管线：`video_converter.js` 收尾时若 `settings.format==='eclipsa'` 且编码器为 HEVC(x265/nvenc)，
  自动调用 `attachSt2094_50()` 附加动态元数据（AV1 则回退 HDR10 并提示）；`main.js` 透传 format。
- UI：`views/video.html`「编码与性能 → 输出格式」选择器（HDR10 / Eclipsa Video），选 AV1 自动切回 HEVC。
- 端到端验证：原始 MKV → `convertVideoFrames(..., format:'eclipsa')` →
  `D:\video\video_sdr\bg_waifu2x_2x_2n_mp4_Eclipsa_3rd.mp4`（7.73MB），master ffprobe 识别
  `HDR Dynamic Metadata SMPTE2094-50`（baseline 14691/13763/13233），静态校验全绿。
- **最终 UI 设计（用户拍板，2026-08）**：「输出格式（HDR10 / Eclipsa）」放「编码与性能」；
  选 Eclipsa 后，位于「画质与色调」下方的 **EV 面板（分窗策略 scene/uniform + 每窗数）**才显示；
  **参考白不单独设置，自动跟随「画质与色调 → 白点」** —— 参考白与主画面白点是同一物理锚点，
  保持一致，避免元数据锚点与画面错位（不允许界面自行解耦引发冲突）。

## P0 最小验证（已完成，2026-08，24/24 通过）

见 `poc/README.md`。一句话：

- **能编能解能注能查**：Annex C 编码/解码字节一致，字节向量命中 `0000`/`00404E2080`/`00800BE5`；
  `trace_headers` 独立确认 SEI = `User Data Registered ITU-T T.35`（country `B5`/provider `0090`），
  载荷 `00 40 4E 20 80` 逐字节一致；`-c copy` 封进 MP4 后 SEI 仍在码流内。
- **实证坑**：项目自带 **gyan essentials 9.0 的 ffprobe 不导出 2094-50 side data**（HEVC/AV1 均不），
  与方案文档“风险 7”一致；脚本已做成能力自适应（不支持的构建自动跳过语义断言，换参考构建自动恢复）。
- 参考构建（BtbN master）下载超时未果，属“可选加强项”，不影响“编码/注入可行”的结论。

## 下一步建议

1. 确认“需要你确认的开放问题”（上方）；
2. ✅ **P0 已完成**（`poc/st2094_50.js` + `poc/verify_st2094_50.js`，24/24 通过）；
3. 可选加强：下载支持 2094-50 的参考 ffmpeg（BtbN/git 构建）后设 `FFPROBE_BIN/FFMPEG_BIN` 重跑
   `poc/verify_st2094_50.js`，补上“ffprobe 语义参照比对”这层；
4. **P1 建议**：把逐窗统计接进现有视频链路（Hbaseline=log2(峰值/参考白)），
   用 `st2094_50.js` 生成“逐窗动态元数据”并按 AUD 注入 → `-c copy` remux → trace_headers/ffprobe 校验。
