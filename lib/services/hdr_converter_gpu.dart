// =====================================================================
// 平台自适应导出 — hdr_converter_gpu
//
// 条件编译：Dart 根据可用库自动选择实现文件。
//   桌面端 (dart.library.io 可用) → hdr_converter_gpu_io.dart
//   Web 端  (dart.library.js  可用) → hdr_converter_gpu_web.dart
//   其他    → hdr_converter_gpu_stub.dart
// =====================================================================
export 'hdr_converter_gpu_stub.dart'
    if (dart.library.io) 'hdr_converter_gpu_io.dart'
    if (dart.library.js) 'hdr_converter_gpu_web.dart';
