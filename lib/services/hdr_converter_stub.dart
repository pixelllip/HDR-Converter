import 'dart:typed_data';
import '../models/conversion_settings.dart';
import 'hdr_converter_platform.dart';

/// SDR 转 HDR 转换器 — 存根实现
///
/// 当平台既不是桌面端也不是 Web 端时使用（当前不会实际使用）。
class HdrConverter implements HdrConverterPlatform {
  HdrConverter._();
  static final HdrConverter _instance = HdrConverter._();

  /// 获取单例实例
  static HdrConverter get instance => _instance;

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

  @override
  String getOutputExtension(ConversionSettings settings) {
    throw UnsupportedError('当前平台不支持 HDR 转换');
  }

  @override
  void openHdrPreview(Uint8List pngBytes) {}

  @override
  void dismissHdrPreview() {}
}
