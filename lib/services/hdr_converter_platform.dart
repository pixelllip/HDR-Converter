import 'dart:typed_data';
import '../models/conversion_settings.dart';

/// HDR 转换器平台抽象接口
///
/// 桌面端（io）使用纯 Dart image 包实现，
/// Web 端使用纯 Dart image 包实现。
abstract class HdrConverterPlatform {
  /// 初始化转换器（加载 ICC 配置文件等）
  Future<void> initialize();

  /// 将 SDR 图像转换为 HDR 图像，返回处理后的图像字节
  Future<Uint8List> convertSdrToHdr({
    required Uint8List inputBytes,
    required ConversionSettings settings,

    /// 进度回调 0.0 ~ 1.0
    void Function(double progress)? onProgress,
  });

  /// 获取预览图像（低分辨率快速处理）
  Future<Uint8List> convertForPreview({
    required Uint8List inputBytes,
    required ConversionSettings settings,

    /// 进度回调 0.0 ~ 1.0
    void Function(double progress)? onProgress,
  });

  /// 获取输出文件扩展名
  String getOutputExtension(ConversionSettings settings);

  /// 打开 HDR 预览
  ///
  /// [x], [y]: 预览区域在窗口中的位置 (物理像素, 仅桌面端)
  /// [w], [h]: 预览区域尺寸 (物理像素, 仅桌面端)
  void openHdrPreview(
    Uint8List pngBytes, {
    int x = 0,
    int y = 0,
    int w = 0,
    int h = 0,
  });

  /// 关闭 HDR 预览覆盖层
  void dismissHdrPreview();
}
