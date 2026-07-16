import 'dart:typed_data';
import '../models/conversion_settings.dart';
import 'hdr_converter_gpu.dart';
import 'hdr_converter_platform.dart';

/// SDR 转 HDR 转换器 — 存根实现
///
/// 当平台既不是桌面端也不是 Web 端时使用（当前不会实际使用）。
class HdrConverter implements HdrConverterPlatform {
  HdrConverter._();
  static final HdrConverter _instance = HdrConverter._();

  /// 获取单例实例
  static HdrConverter get instance => _instance;

  /// GPU 加速是否可用 (存根始终不可用)
  bool get isGpuAvailable => false;

  /// GPU 后端名称
  String get gpuBackendName => 'None';

  /// GPU 初始化错误消息
  String? get gpuErrorMessage => null;

  /// 最近一次导出实际使用的后端
  String lastUsedBackend = 'CPU';

  @override
  Future<void> initialize() async {
    throw UnsupportedError('当前平台不支持 HDR 转换');
  }

  @override
  Future<Uint8List> convertSdrToHdr({
    required Uint8List inputBytes,
    required ConversionSettings settings,
    void Function(double progress)? onProgress,
  }) {
    throw UnsupportedError('当前平台不支持 HDR 转换');
  }

  @override
  Future<Uint8List> convertForPreview({
    required Uint8List inputBytes,
    required ConversionSettings settings,
    void Function(double progress)? onProgress,
  }) {
    throw UnsupportedError('当前平台不支持 HDR 转换');
  }

  /// GPU 处理后返回 RGBA (存根返回 null)
  Future<ProcessedImage?> processImageBytes(
    Uint8List inputBytes,
    ConversionSettings settings,
  ) async => null;

  @override
  String getOutputExtension(ConversionSettings settings) {
    throw UnsupportedError('当前平台不支持 HDR 转换');
  }

  @override
  void openHdrPreview(Uint8List pngBytes) {}

  @override
  void dismissHdrPreview() {}
}
