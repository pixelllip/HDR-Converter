// =====================================================================
// 平台自适应导出
//
// 条件编译：Dart 根据可用库自动选择实现文件。
//   桌面端 (dart.library.io 可用) → hdr_converter_io.dart
//   Web 端  (dart.library.js  可用) → hdr_converter_web.dart
//   其他    → hdr_converter_stub.dart（抛 UnsupportedError）
// =====================================================================
export 'hdr_converter_stub.dart'
    if (dart.library.io) 'hdr_converter_io.dart'
    if (dart.library.js) 'hdr_converter_web.dart';
