# P0 最小验证（poc）结果

> 目的：用最少的代码证实“本项目能生成 & 注入 SMPTE ST 2094-50 (Application #5) 元数据”这件事可行。
> 结论：**可行且已跑通**（2026-08，自测 24/24 通过）。

## 文件

| 文件 | 作用 |
|---|---|
| `st2094_50.js` | Annex C 二进制**编码/解码** + T.35 载荷 + HEVC Prefix/Suffix_SEI NAL（含 EBSP）/ |
| `hevc_inject.js` | 按 AUD（nal_type=35）为每个 access unit 注入 SEI（支持 after-aud 前缀 / before-aud 后缀） |
| `verify_st2094_50.js` | 主验证脚本（A 自测 + B ffprobe + B2 trace_headers + C MP4 透传 + C2 结构断言） |
| `_debug_probe.js` | 定向诊断（HEVC 前缀/后缀、AV1 元数据 OBU 是否被本机构建解析） |

## 运行

```bash
node verify_st2094_50.js
# 用参考/新构建（可选）：设置环境变量后运行
#   $env:FFMPEG_BIN=...\ffmpeg.exe ; $env:FFPROBE_BIN=...\ffprobe.exe
```

> 脚本会自动用项目自带 ffmpeg 9.0（`../../../backend/ffmpeg/`)生成 64x64 的 HEVC 测试源并就地注入。

## 本次验证结果（2026-08，本地 gyan essentials 9.0）

| 段 | 内容 | 结果 |
|---|---|---|
| A | Annex C 编码→解码→再编码 **字节一致**；字节向量命中规范推导值：`0000` / `00404E2080` / `00800BE5` | 8/8 ✅ |
| B2 | **trace_headers**（独立位流解析器）确认 SEI：`User Data Registered ITU-T T.35`、country `0xB5`、provider `0x0090`、载荷字节 `00 40 4E 20 80` | 4/4 ✅ |
| C | 注入后 `ffmpeg -c copy -movflags +faststart` 封进 MP4，流为 `HEVC` | ✅ |
| C2 | 对 MP4 再跑 trace_headers：SEI 经 `-c copy` 后**仍在码流内**（= 位于 mdat，moov 无需改） | ✅ |
| B（语义） | ffprobe `-show_frames` 解析出 2094-50 字段并与编码值比对 | 本构建不支持 → SKIP（见下） |

## 重要发现（实证）

1. **我们的编码/注入字节完全规范**：自研 round-trip 字节一致 + trace_headers 独立解析逐字节相符。
2. **项目自带的 ffmpeg 9.0 essentials 构建不导出 2094-50 side data**：同样注入的 SEI，
   HEVC 前缀/后缀、AV1 元数据 OBU 三种载体，ffprobe `-export_side_data 1 -show_frames`
   都不显示 App5 字段（x265 自带的 `User Data Unregistered` SEI 却能显示 → 说明 SEI→side data
   机制在，缺的是 registered-T.35/2094-50 这条分支）。这与 02 文档“风险 #7：本地构建完整性”一致。
3. 因此脚本对 B 段做了**能力自适应**：检测到 ffprobe 不导出时自动跳过语义断言（保持 exit 0），
   换用支持 2094-50 的 ffmpeg（如 BtbN master、或更新后的正式版）时自动恢复全量语义比对。
   参考构建（BtbN master ~170MB）本次下载超时未果，可日后补跑：
   ```bash
   # 设置好 FFPROBE_BIN/FFMPEG_BIN 后直接 node verify_st2094_50.js
   ```

## P1（2026-08）：真实视频注入 ST 2094-50 动态元数据

- 脚本：`inject_st2094_50_video.js <in.mp4> <out.mp4> [windowCount]`
- 流程：signalstats 逐帧 YMAX → PQ EOTF → 每窗 MaxCLL → `Hbaseline=log2(MaxCLL/参考白)` →
  C.3.8 参考白配方编码 → mp4→annexb(补 AUD)→按 AUD 注入→remux(`-tag:v hvc1 -avoid_negative_ts make_zero`)
  →`mp4_hdr.js` 补回 mdcv/clli。
- 踩坑：裸流重封会让开头 PTS 变负、丢 3 帧 → 必须 `-avoid_negative_ts make_zero`（已修，200 帧全保留）。
- 实测（`bg_waifu2x_2x_2n_mp4_HDR10_frames.mp4`，200 帧，3 窗，参考白=203）：

| 窗 | 帧范围 | MaxCLL≈ | Hbaseline(档) | raw | 载荷(不含 T.35 头) |
|---|---|---|---|---|---|
| 0 | 0–65 | 562 尼特 | 1.4691 | 14691 | `00 40 39 63 80` |
| 1 | 66–132 | 527 尼特 | 1.3763 | 13763 | `00 40 35 C3 80` |
| 2 | 133–199 | 508 尼特 | 1.3233 | 13233 | `00 40 33 B1 80` |

- 自验：V1 逐 AUD 回读 200/200，逐窗 baseline 与期望一致；V2 最终 mp4 检出 200 条
  `User Data Registered ITU-T T.35` 且 remux 不丢；`tests/verify_hdr_metadata.js` 静态校验全绿。
- **金标准确认（2026-08，BtbN master N-126229-gf101fce22d-20260820）**：该最新官方
  `ffprobe -export_side_data 1 -show_frames` 在成品上识别出
  `side_data_type: HDR Dynamic Metadata SMPTE2094-50`，并逐帧解析出 3 窗
  `baseline_hdr_headroom` = 14691 / 13763 / 13233（1.469 / 1.376 / 1.323 档），
  与注入值完全一致 → **工具显示层闭环**。
- 成品：`D:\video\video_sdr\bg_waifu2x_2x_2n_mp4_Eclipsa_ST2094_50.mp4`
  （静态 HDR10 + 3 窗动态 ST 2094-50 共存）。

### “填什么数值”参考
- **参考白锚点**：默认 `203 cd/m²`（`has_custom=0`，BT.2408/规范默认）；自定义时
  `hdr_reference_white = round(Lwhite×15)` 语义（草案 HTML 疑 ÷15，实施前以正式版核对）。
- **每窗 headroom**：`Hbaseline = log2(窗MaxCLL / 参考白)`；其余 alternate/增益曲线交给
  **C.3.8 配方**自动生成（Nalt=2、Halt,0=0、Halt,1=log2(8/3)·min(Hb/log2(1000/203),1)、
  Ncp=8 贝塞尔 κ=0.65、ComponentMix 只走 Max 通道）。**Htarget 是显示端参数，不入码流**。

## 最有用的两条结论（对主方案）

- **HEVC 带内 SEI 注入 + trace_headers/ffprobe 校验** 这条链路骨架已验证可行，工程量很小。
- **不要在产品里依赖“本地 ffmpeg 已支持 2094-50 解析”**：gyan essentials 9.0 不行；
  若要 ffprobe/预览侧能看到，需换支持 2094-50 的构建，或自己写解析（本模块已具备解码能力，
  可直接作为校验器）。
