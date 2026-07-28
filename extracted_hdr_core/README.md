# HDR 图片显示核心代码

从 HDR Show (Electron) 项目中摘出的核心 HDR 显示功能代码。

## 文件结构

```
extracted_hdr_core/
├── README.md                # 本文件
├── hdr-core.js              # 核心 HDR 处理逻辑 (纯JS，无任何依赖)
├── hdr-renderer.js           # HDR Canvas 渲染与加载器
├── hdr-icc-parser.js         # ICC Profile 解析 (色彩空间检测)
├── hdr-gainmap.js            # Gain Map (Ultra HDR) 解析
├── hdr-demo.html             # 独立演示页面 (浏览器中可直接运行)
└── demo.js                   # 演示页面脚本
```

## 核心模块说明

### 1. `hdr-core.js` - HDR 数学基础
- **sRGB <-> Linear 色彩空间转换**: `srgbToLinear()`, `linearToSrgb()`
- **RGBE (.hdr) 解码器**: `decodeRgbe()` — 解析 Radiance HDR 格式
- **色调映射函数**: Reinhard、ACES、Filmic 三种 Tone Mapping
- **Gamma 校正**: `gammaCorrection()`
- **曝光控制**: `applyExposure()`
- **HDR 像素渲染**: `renderHdrToCanvas()`
- **Gain Map 重建**: `readCanvasPixelsLinear()`, `readGainMapPixels()`, `applyGainMap()`

### 2. `hdr-icc-parser.js` - ICC Profile 色彩空间检测
- **HDR 色彩空间关键词**: BT.2020, Display P3, scRGB, PQ/ST.2084, HLG 等
- **ICC Profile 解析**: `parseIccProfile()`
- **JPEG ICC 提取**: `extractJpegIccData()` — 从 APP2 段提取
- **PNG ICC 提取**: `extractPngIccData()` — 从 iCCP 块提取
- **HDR 检测**: `detectHdrFromIcc()`

### 3. `hdr-gainmap.js` - Gain Map (Ultra HDR) 解析
- **JPEG Gain Map 提取**: `extractJpegGainMap()` — 通过 MPF (Multi-Picture Format)
- **PNG Gain Map 提取**: `extractPngGainMap()` — 通过 eXIf/MPF 或自定义块
- **MPF 解析**: `findMpfPointer()`, `parseMpf()`
- **Gain Map 检测入口**: `detectAndExtractGainMap()`

### 4. `hdr-renderer.js` - 渲染与加载器
- **Canvas 初始化**: 支持 display-p3 色彩空间
- **HDR 显示检测**: `detectHdrDisplay()`
- **各类图片加载**:
  - `loadHdrRgbe()` — .hdr 文件
  - `loadGainMapImage()` — Gain Map (Ultra HDR)
  - `loadIccHdrImage()` — ICC HDR (img 标签原生)
  - `loadStandardImage()` — 普通图片
  - `loadExrImage()` — OpenEXR
- **缩放控制**

## 关键流程

### RGBE (.hdr) 文件显示流程
```
.hdr 文件 → decodeRgbe() → Float32Array(RGB) → 色调映射 → Canvas
```

### Gain Map (Ultra HDR) 显示流程
```
JPEG/PNG → detectAndExtractGainMap() → SDR图像 + Gain Map权重图
→ applyGainMap() → Float32Array(HDR) → 色调映射 → Canvas
```

### ICC Profile HDR 显示流程
```
JPEG/PNG → detectHdrFromIcc() → 检测到BT.2020/P3等 → img标签原生显示
```

## 使用方式

浏览器演示: 打开 `hdr-demo.html` 即可。
