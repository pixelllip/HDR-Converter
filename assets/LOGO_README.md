# HDR Converter Logo 使用说明

本目录包含为 HDR Converter 桌面应用程序设计的完整 logo 套件。

## 📁 文件清单

### 矢量源文件 (SVG - 推荐，可无损缩放)
| 文件 | 用途 | 尺寸 |
|------|------|------|
| `logo.svg` | 主 logo (图标+文字) | 512 × 512 |
| `logo-icon.svg` | 纯图标版本 | 512 × 512 |
| `logo-horizontal.svg` | 横向布局 (图标+文字) | 800 × 200 |

### 位图文件 (PNG - 已导出)
**主 logo (图标+文字)**
- `logo-512.png` (512 × 512) — 应用商店、Retina 屏幕
- `logo-256.png` (256 × 256) — 高分辨率
- `logo-128.png` (128 × 128) — 工具栏
- `logo-64.png` (64 × 64) — 列表/菜单

**纯图标**
- `logo-icon-512.png` (512 × 512) — 大尺寸应用图标
- `logo-icon-256.png` (256 × 256)
- `logo-icon-128.png` (128 × 128)
- `logo-icon-64.png` (64 × 64)
- `logo-icon-32.png` (32 × 32) — 任务栏
- `logo-icon-16.png` (16 × 16) — favicon

**横向布局**
- `logo-horizontal-800.png` (800 × 200) — 网站头部
- `logo-horizontal-400.png` (400 × 100) — 文档/小尺寸

## 🎨 设计规范

### 配色
| 颜色 | 色值 | 用途 |
|------|------|------|
| 深蓝 | `#1E3A8A` | 主背景渐变起点 |
| 深紫 | `#4C1D95` | 主背景渐变中点 |
| 亮紫 | `#7C3AED` | 主背景渐变终点 |
| 金黄 | `#FCD34D` | 高光渐变起点 |
| 橙色 | `#FB923C` | 高光渐变中点 |
| 深橙 | `#F97316` | 高光渐变终点 |

### 设计理念
- **HDR 字母组合**：使用粗壮的字母"HDR"作为视觉主体，强调品牌核心
- **暖色 D**：中间的 D 使用金橙渐变，象征 HDR 高动态范围的"高光"
- **冷色边框**：深蓝到紫色渐变，象征 HDR 的"暗部"，体现完整动态范围
- **半透明圆环**：表现光的层次，呼应 HDR 明暗对比
- **大圆角矩形**：现代应用图标形态，适配 macOS/Windows/Linux 各平台

## 💻 Electron 应用集成

### 在 `main.js` / `package.json` 中使用

**package.json 配置：**
```json
{
  "name": "hdr-converter",
  "productName": "HDR Converter",
  "version": "1.0.0"
}
```

**BrowserWindow 图标：**
```javascript
const { app, BrowserWindow } = require('electron');
const path = require('path');

function createWindow() {
  const win = new BrowserWindow({
    width: 1200,
    height: 800,
    icon: path.join(__dirname, 'assets', 'logo-256.png'),  // Windows/Linux
    title: 'HDR Converter'
  });
  // ...
}
```

**应用图标 (Windows .ico / macOS .icns)：**
建议使用 [electron-icon-maker](https://www.npmjs.com/package/electron-icon-maker) 或
[squoosh/cli](https://github.com/GoogleChromeLabs/squoosh) 从 `logo-icon-512.png` 生成。

### 在 HTML 页面中使用

```html
<!-- 横向 logo (页面头部) -->
<img src="assets/logo-horizontal-400.png" alt="HDR Converter" height="60">

<!-- SVG 内嵌 (支持 CSS 样式控制) -->
<svg style="height: 64px;">
  <use href="assets/logo.svg#root"/>
</svg>

<!-- favicon -->
<link rel="icon" type="image/png" sizes="32x32" href="assets/logo-icon-32.png">
<link rel="icon" type="image/png" sizes="16x16" href="assets/logo-icon-16.png>
```

## 📦 重新生成 PNG

如需重新生成 PNG (例如修改 SVG 后)：

```bash
node build/generate-logo-png.js
```

依赖：`sharp` (已在 `node_modules` 中)。

## 🎨 设计预览

打开 `design_previews/logo-preview.html` 在浏览器中查看所有方案的实时预览。
