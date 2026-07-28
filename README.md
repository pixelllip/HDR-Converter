# HDR Converter Electron

将普通 SDR（标准动态范围）图片转换为 HDR（高动态范围）图片的 Electron 桌面应用。

## 运行方式

```bash
cd hdr_electron
npm install
npm start
```

## 功能

- SDR → HDR 转换（HDR PNG / Ultra HDR JPEG）
- ICC BT.2020 配置文件嵌入
- 实时参数调节（HDR 强度、明暗、伽马校正）
- 实时预览 SDR / HDR 对比
- 拖拽 / 点击选择图片
- 批量转换

## 项目结构

```
hdr_electron/
├── backend/
│   ├── hdr_converter.js     # HDR 转换核心（sharp）
│   └── 2020_profile.icc     # BT.2020 ICC 配置文件
├── extracted_hdr_core/      # HDR 核心 JS 库（可复用）
├── main.js                  # Electron 主进程
├── preload.js               # Electron 预加载脚本
├── hdr_viewer.html          # 前端 UI
└── package.json             # Node.js 依赖
```

## 技术原理

- 使用 BT.2020 ICC 色彩配置文件
- sRGB → 线性 → HDR 色调映射 → ICC 元数据注入
- Node.js sharp 处理图像编码
