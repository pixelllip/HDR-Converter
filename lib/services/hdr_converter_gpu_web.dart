import 'dart:async';
import 'dart:typed_data';
import 'package:image/image.dart' as img;
import '../models/conversion_settings.dart';

/// GPU 处理后的图像数据 (不含编码)
class ProcessedImage {
  final Uint8List rgba;
  final int width;
  final int height;
  ProcessedImage({required this.rgba, required this.width, required this.height});
}

/// GPU 加速的 HDR 转换器 — Web 端存根
///
/// Web 平台不支持 GPU 加速，所有方法均返回不可用。
class GpuAcceleratedConverter {
  GpuAcceleratedConverter();

  static Future<void> yieldNow() => Future<void>.delayed(Duration.zero);

  /// Web 端不可用
  bool get isGpuAvailable => false;

  /// 后端名称
  String get backendName => 'None';

  /// 错误消息
  String? get errorMessage => null;

  Future<bool> tryInitialize() async => false;

  Future<ProcessedImage?> processImageBytes(
    Uint8List inputBytes,
    ConversionSettings settings,
  ) async => null;

  Future<Uint8List?> convertWithGpu({
    required Uint8List inputBytes,
    required ConversionSettings settings,
  }) async => null;

  Future<Uint8List?> convertAndEncode({
    required Uint8List inputBytes,
    required ConversionSettings settings,
    Uint8List? iccProfile,
  }) async => null;

  void dispose() {}
}
