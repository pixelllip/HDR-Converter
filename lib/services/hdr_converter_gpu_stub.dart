import 'dart:typed_data';
import '../models/conversion_settings.dart';

/// GPU 处理后的图像数据 (不含编码)
class ProcessedImage {
  final Uint8List rgba;
  final int width;
  final int height;
  ProcessedImage({required this.rgba, required this.width, required this.height});
}

/// GPU 加速的 HDR 转换器 — 存根
///
/// 当前平台不支持 GPU 加速。
class GpuAcceleratedConverter {
  GpuAcceleratedConverter();

  bool get isGpuAvailable => false;
  String get backendName => 'None';
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
