# Ultra HDR 增益图下采样回归对比（nte_bg_0.png）

**目的**：用一张真实业务图（`D:\Neverness To Everness\NTELauncher\ResFilesM\1289\bgimgs\bg_0.png`，1920×1080）量化对比三步演进对 Ultra HDR 增益图边缘轮廓的影响。

**输入产物**（均在 `tests/`）：

| 文件 | 说明 | 来源 |
|---|---|---|
| `nte_bg_0_input.png` | 主图（1920×1080，从 NTELauncher 拷贝） | 用户提供 |
| `gainmap_step1_fullres_bilinear.png` | 旧：全分辨率 mask + 双线性 decimation（480×270） | `cargo run --example dump_gainmap_compare` |
| `gainmap_step1_fullres_box.png` | 第一步：全分辨率 mask + box decimation | 同上 |
| `gainmap_step2_lowres.png` | 第二步：低分辨率 mask/gain（硬阈值） | 同上 |
| `gainmap_step3_lowres_maskblur.png` | 第三步（当前主链路）：低分辨率 mask + 3×3 高斯软阈值 | 同上 |
| `gainmap_step1_bilinear_vs_step2_diff.png` | step1-bilinear vs step2 差值 | 同上 |
| `gainmap_step1_bilinear_vs_step3_diff.png` | step1-bilinear vs step3 差值（终极对比） | 同上 |
| `gainmap_step2_vs_step3_diff.png` | step2 → step3 差值（软阈值过渡） | 同上 |
| `nte_bg_0_ultrahdr_step3.jpg` | 完整 Ultra HDR（第三步主链路生成） | `hdrconv -f ultra-hdr` |

**数值概要**（`gainmap_compare_report.json`）：

| 对比 | vs step3 max | vs step3 mean | vs step3 >32 占比 | RMSE |
|---|---|---|---|---|
| step1-bilinear（旧）vs step3 | 248 | 6.43 | 6.85% | 20.91 |
| step1-box（第一步）vs step3 | 152 | 4.39 | 4.55% | 12.99 |
| step2（第二步）vs step3 | 170 | 4.43 | 4.77% | 13.93 |
| **Ultra HDR 抽出增益图 vs step3** | — | — | — | **0.336** ✅ |

> 关键事实：抽出的 Ultra HDR 内嵌增益图与 `gainmap_step3_lowres_maskblur.png` 几乎完全一致（RMSE 0.336）——证明已上线的主链路就是第三步。

**最信息量剖面**（y=176, x=364..424，穿过亮斑边缘）：

```
step1-bilinear: 0,0,0, 148,193,195,209,192,192,198,148,229,212,215,214,213,211,210,0,0
step1-box     : 0,0,0, 136,186,206,205,150,196,161,206,216,214,213,213,211,211,107,0,0
step2         : 0,0,0, 132,186,206,206,153,195,164,207,216,215,215,214,212,212, 92,0,0
step3         : 0,0, 27,83,155,193,204,201,197,203,209,222,230,229,228,227,191,125,38,0
```

| | 最大相邻跳变 | 软爬升宽度 |
|---|---|---|
| step1-bilinear（旧） | 210 | 0（单步跳变） |
| step1-box（第一步） | 137 | 0 |
| step2（第二步） | 138 | 0 |
| **step3（当前）** | **87** | **~3 低分辨率像素（≈12 主图像素）** |

step3 在亮带前后各出现 ~3 个低分辨率像素宽的渐变带，进入带前 0→27→83→155→193、退出带后 191→125→38→0；这是"硬阈值 → 高斯软阈值"的直观表现——过渡带变宽且单调无过冲。

**对比脚本输出片段**（节选 y=176）：

```
===== 全图 |差| 统计（vs step3，当前主链路） =====
step1-bilinear vs step3 : max=248 mean=6.43 rmse=20.91
step1-box      vs step3 : max=152 mean=4.39 rmse=12.99
step2          vs step3 : max=170 mean=4.43 rmse=13.93
```

## 三步演进总结

| 步骤 | 关键改动 | 改善点 | 残余问题 |
|---|---|---|---|
| **原状** | 全分辨率逐像素 mask + 2-tap 双线性 decimation | — | 锐阶跃→混叠→DCT 振铃→4× 上采样后 4~8px 毛边 |
| **第一步** | decimation 改 4×4 box 平均 | 消除混叠/锐阶跃 | 5% 像素差异 >32，残余毛边 |
| **第二步** | 先 box 下采样主图到 1/4，再算 mask/gain | 主图→增益图尺度一致；过渡带宽度与解码端 4× 上采样匹配 | 低分辨率硬阈值 → 1 像素阶跃 |
| **第三步** | mask 再做 3×3 高斯模糊 | 硬阈值 → 软阈值；过渡带扩到 2~3 低分辨率像素（8~12 主图像素）；无过冲 | 基本无可见毛边 |

## 复现命令

```bash
# 1) 生成三步对比产物（已在 repo 内）
cargo run --example dump_gainmap_compare -- tests/nte_bg_0_input.png tests/

# 2) 生成完整 Ultra HDR（当前主链路）
backend/rust/target/debug/hdrconv.exe tests/nte_bg_0_input.png -o tests/nte_bg_0_ultrahdr_step3.jpg -f ultra-hdr

# 3) 跑对比脚本
node tests/compare_nte_gainmaps.js

# 4) Rust 全套回归
cd backend/rust && cargo test --test regression -- --nocapture
```
