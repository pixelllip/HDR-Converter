# GitHub Copilot 运行约束

## 命令行执行规范
- 在 Windows 环境下，绝对不要在命令末尾添加 `| Select-Object` 或 `2>&1` 等 PowerShell 特有的管道截断命令。
- 如果需要编译，请直接输出原生的 `cmake --build ...` 完整命令。