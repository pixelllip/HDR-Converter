# 参考来源与合规备忘（SOURCES）

> 本实验围绕 **SMPTE ST 2094-50（Application #5, Broadcast）** 展开。
> 本文档只记录“来源在哪里、怎么看、许可上要注意什么”，**不在本实验文件夹内复制 / 托管 SMPTE 草案原文**（见下方许可条款）。

## 1. 官方来源（SMPTE 公开仓库）

| 项 | 值 |
|---|---|
| 仓库 | https://github.com/SMPTE/st2094-50 （SMPTE 官方，公开） |
| 默认分支 | `main` |
| 托管内容 | 委员会草案（Committee Draft）文档本体：`10e-st-2094-50-cd-2026-02-23-draft.zip` |
| 草案正文 | ZIP 内含 `index.html`（HTML 版）与 `SMPTE-ST-2094-50-2026-02-23-…-Application-#5.pdf`（PDF 版）及全部插图 |
| 反馈入口 | https://github.com/SMPTE/st2094-50/issues 或 10E 委员会 <10e-chair@smpte.org> |
| 私有讨论区 | `https://github.com/SMPTE/st2094-50-private`（SMPTE 标准社区成员可见） |

## 1b. Eclipsa Video 官方来源与官方事实（2026-08 已核实）

| 项 | 值 |
|---|---|
| 官方公告（HTML） | https://eclipsamedia.org/wp-content/uploads/2026/05/EclipsaVideo-release-final.html |
| 官方公告（PDF） | https://eclipsamedia.org/wp-content/uploads/2026/05/EclipsaVideo-release-final.pdf |
| 官网 | https://eclipsamedia.org/ |
| Chrome 官方博客 | https://blog.google/chromium/bringing-a-clearer-more-consistent-hdr-video-experience-to-chrome/ |

**官方确认的事实（据 2026-05 公开材料转述）：**

1. **Eclipsa Video 是基于 SMPTE ST 2094-50 的开源视频标准**，由 Google / Apple / NBCUniversal
   专家参与的 SMPTE 规范（即本实验精读的草案家族）；2026-05-26 宣布由 **HDR10+ Technologies LLC**
   负责其项目管理/认证（继 2025-10 启动的 Eclipsa Audio 认证之后）。
2. 官方对与 HDR10+ 的关系表述为 **“seamlessly integrates with the broadly supported HDR10+ standard”**，
   并说明 **“Devices certified for both standards may utilize the name 'Eclipsa Video powered by HDR10+'”**
   ——即“无缝整合”指生态共存/组合品牌，**不是解码等价**；设备需**分别认证两个标准**。
   （技术差异见 `02_feasibility_and_plan.md` §0.1。）
3. Chrome 官方博客（2026-05-14）：Chrome 将支持 **finalized** 的 ST 2094-50（“coming in an upcoming release”），
   并把该标准描述为“Reference White 锚点 + Headroom-Adaptive Gain Curves”两块元数据——与 PCD2 一致。
4. 许可/测试：官方称“licensing and testing Eclipsa Video”信息可从 www.eclipsamedia.org 获取。

> 推论：除 PCD 外，可预期 **finalized 版 ST 2094-50** 即将正式发布（Chrome 公告已用 “finalized” 措辞），
> 正式实施前以最新官方版为准；Eclipsa Video 本身的“承载/封装细则”官方尚未公开，仍属待确认项。

## 2. 版本时间线（2026-08 观察）

- 本仓库 README 标注的草案为 **PCD2（Second Public Committee Draft）**，正文日期 `2026-02-23`，公开评论期截止 **2026-03-16**。
- 仓库 HEAD（本次克隆）提交：`bb8316a — 2026-05-21 — Update README.md`，内容仍为 PCD2。
- 另有活跃特性分支：`feature/pcd1`、`pcd-info-cleanup`；开放 PR：#36、#69。
- **结论**：这是“未发布标准”的委员会草案，非最终 SMPTE 标准；正式规范或更新 PCD 未来会另行发布。
  **实施前必须重新核对仓库/官网是否有更新版本，勿把 PCD2 当作定稿。**

## 3. 草案结构速查（HTML 版目录，便于精读）

- 1 Scope / 2 Conformance / 3 规范性引用 / 4 术语定义
- 5 数学记号
- 6 数据结构：6.1 颜色体积变换 / 6.2 headroom 自适应色调映射（HATM）/ 6.3 颜色增益函数 / 6.4 分量混合函数 / 6.5 增益曲线
- 7 应用约束：7.1 元数据集 / 7.2 元数据集约束 / **7.3 元数据携带（T.35）**
- Annex A：颜色体积变换计算（资料性）
- Annex B：示例（简单色调映射 / 双 alternate / 逆色调映射 / 无色调映射）
- Annex C：**二进制编码（规范性）**：C.2 语法、C.3 语义（含 C.3.8 参考白自适应色调映射计算、C.3.9 PCHIP 斜率）

## 4. 相邻标准 / 引用（同族）

| 编号 | 用途 | 与本项目关联 |
|---|---|---|
| SMPTE ST 2094-1:2016 | 颜色体积变换元数据核心组件（Application #5 是其特化） | 定义了 TimeInterval/ProcessingWindow 等公共结构 |
| **ISO 21496-1:2025** | 增益图元数据（动态范围转换） | 与本项目 **Ultra HDR JPEG 增益图**同一家族；ST 2094-50 的“参考白/headroom/alternate”概念与其 3.x 条款同源 |
| ICC Adaptive Gain Curve (2025) | 自适应增益曲线 | Annex C 的“分量混合函数/增益曲线”与其同构（标准内直接引用） |
| ITU-R BT.2408-8 | HDR 制作操作指导 | **参考白 203 尼特**同源；项目默认峰值的依据 |
| MovieLabs SDR→HDR 最佳实践 | SDR→HDR 映射 | 项目直接转换链路参照 |
| ITU-R BT.2100-3 | PQ/HLG、相对线性浮点表示 | 本项目编码输出使用 BT.2020/PQ |
| ITU-T T.35 | 非标准设施代码分配 | **7.3 元数据携带**所依赖的标识机制 |
| ISO/IEC 14496-12:2022 | ISO BMFF（MP4） | 容器携带时需要的盒结构基础 |

## 5. 许可与合规（务必注意）

- 仓库 `LICENSE.md`：**版权归 SMPTE 所有，未经 SMPTE 书面许可，不得以任何方式复制本材料**。
  因此：
  - 本实验文档只做**总结、转述与工程量描述**，不整段转载草案正文、插图或公式文本；
  - 本实验文件夹内**不内置** ZIP/PDF/HTML 草案副本；
  - 如后续需要把规范实现进产品（参考实现代码、位级编码器），**先与 SMPTE/10E 确认许可与版税条款**。
- `PATENTS.md`：SMPTE 声明不负责识别本文件可能涉及的**专利权利**；商业落地前建议做专利排查。
- `CONFIDENTIALITY.md`：本公开仓库内容不属于 SMPTE 机密信息，可按公开材料研读。

## 6. 本机查阅痕迹（仅供本次实验内部使用）

- 临时克隆：`%TEMP%\dsh_st2094_clone`（含 PCD2 ZIP）
- 临时解包/全文纯文本（已转 `index.txt`）：`%TEMP%\dsh_st2094_doc`
- 以上均为**本机临时副本**，不应提交进项目、不应分发。
