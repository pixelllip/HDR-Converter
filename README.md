# SDR → HDR 转换器

将普通 SDR（标准动态范围）图片转换为 HDR（高动态范围）图片的桌面应用 / Web 工具。

## 功能

- **SDR → HDR 转换**：通过 ICC gain 技术将普通照片转换为 HDR 格式
- **实时预览**：调整参数后即时预览 HDR 效果（Web 端支持浏览器原生 HDR 显示）
- **参数调节**：
  - HDR 强度（1.0–2.0）
  - 微调明暗（0.3–1.5）
  - RGB 通道独立调整（0.0–3.0）
- **输出格式**：
  - **HDR PNG（ICC增益）** — 8-bit PNG + BT.2020 ICC 配置文件，兼容性最佳
  - **AVIF** — 高效压缩格式（仅桌面端）
  - **Ultra HDR JPEG** — 向后兼容标准 JPEG 查看器（仅桌面端）
- **双平台**：Windows 桌面端 + Web 端（GitHub Pages 部署）

## 技术原理

ICC gain 技术将经过 HDR 处理后的 sRGB 像素值嵌入 BT.2020 ICC 配置文件：

```
SDR 输入 → 线性 RGB → 自动伽马 → 通道调整 → 曝光 → Power → sRGB
    ↓
8-bit PNG + BT.2020 ICC 配置文件
    ↓
ICC 感知的查看器/浏览器 → 自动映射到广色域 → HDR 效果
```

## 构建

### 前置条件

- Flutter SDK >= 3.12
- Windows 端：Visual Studio 2022 生成工具（含 C++ 支持）
- Web 端：无额外依赖

### 构建命令

```bash
# 安装依赖
flutter pub get

# Windows 桌面端
flutter build windows --release

# Web 端（GitHub Pages）
flutter build web --release --base-href /
```

或使用项目内构建脚本：

```bash
# CMD
build_all.bat

# PowerShell
.\build_all.ps1
```

## 项目结构

```
lib/
├── main.dart                          # 应用入口
├── models/
│   └── conversion_settings.dart       # 转换设置模型
├── screens/
│   └── home_screen.dart               # 主界面（响应式排版）
├── services/
│   ├── hdr_converter.dart             # 条件导出入口
│   ├── hdr_converter_platform.dart    # 抽象接口
│   ├── hdr_converter_io.dart          # 桌面端实现（ImageMagick FFI）
│   ├── hdr_converter_web.dart         # Web 端实现（image 纯 Dart）
│   ├── hdr_converter_stub.dart        # 存根
│   ├── file_save_helper.dart          # 文件保存（条件导出）
│   ├── file_save_helper_io.dart       # 桌面端：dart:io
│   └── file_save_helper_web.dart      # Web 端：package:web Blob
└── widgets/
    ├── settings_panel.dart            # 设置面板
    └── web/
        └── hdr_viewer.dart            # Web HDR 覆盖层
```

## 技术栈

| 组件 | 桌面端 | Web 端 |
|:--|:--|:--|
| **图像处理** | `image_magick_q8_hdri`（ImageMagick FFI） | `image` 包（纯 Dart） |
| **HDR 显示** | — | `package:web` DOM 原生 `<img>` |
| **文件选择** | `file_selector` | `file_selector` |
| **文件保存** | `dart:io` | `package:web` Blob 下载 |
| **ICC 配置** | ImageMagick 嵌入 | `image` 包 PngEncoder 嵌入 |

## 许可证

MIT

