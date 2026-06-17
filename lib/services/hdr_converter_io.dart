import 'dart:typed_data';
import 'package:flutter/services.dart' show rootBundle;
import 'package:image_magick_q8_hdri/image_magick_q8_hdri.dart';
import '../models/conversion_settings.dart';
import 'hdr_converter_platform.dart';

/// SDR 转 HDR 转换器 — 桌面端实现
///
/// 使用 [image_magick_q8_hdri]（FFI 绑定 ImageMagick C 库）。
class HdrConverter implements HdrConverterPlatform {
  static final HdrConverter _instance = HdrConverter._();

  /// 获取单例实例
  static HdrConverter get instance => _instance;

  HdrConverter._();

  bool _initialized = false;
  Uint8List? _bt2020Profile;

  @override
  Future<void> initialize() async {
    if (_initialized) return;

    ImageMagickFFIPlugin.registerWith();
    _bt2020Profile = (await rootBundle.load(
      'assets/2020_profile.icc',
    )).buffer.asUint8List();
    _initialized = true;
  }

  /// 对 MagickWand 应用 HDR 转换处理（不包含 I/O）
  Future<void> _applyHdrProcessing(
    MagickWand wand,
    ConversionSettings settings,
  ) async {
    // ===== 预处理 =====
    // 移除输入图像可能自带的 ICC 配置文件，避免干扰
    wand.magickProfileImage('*', null);

    // 如果含 Alpha 通道，与黑色背景合并转为不透明 RGB
    if (wand.magickGetImageAlphaChannel()) {
      final bg = PixelWand.newPixelWand();
      bg.pixelSetColor('black');
      wand.magickSetImageBackgroundColor(bg);
      await wand.magickSetImageAlphaChannel(
        AlphaChannelOption.RemoveAlphaChannel,
      );
      bg.destroyPixelWand();
    }

    // 设置为浮点格式以进行 HDR 计算
    wand.magickSetOption('quantum:format', 'floating-point');

    // 转换为线性 RGB 颜色空间（实际转换像素数据）
    await wand.magickTransformImageColorspace(ColorspaceType.RGBColorspace);

    // 自动伽马校正 - 优化动态范围
    await wand.magickAutoGammaImage();

    // 计算总曝光强度
    final totalExposure = settings.totalExposure;

    // 应用 RGB 通道独立调整（使用颜色矩阵，避免 channel mask 的 bug）
    if (settings.rgbAdjustment.red != 1.0 ||
        settings.rgbAdjustment.green != 1.0 ||
        settings.rgbAdjustment.blue != 1.0) {
      final matrix = Float64List.fromList([
        settings.rgbAdjustment.red, 0, 0, 0, 0, // R'
        0, settings.rgbAdjustment.green, 0, 0, 0, // G'
        0, 0, settings.rgbAdjustment.blue, 0, 0, // B'
        0, 0, 0, 1, 0, // A'
        0, 0, 0, 0, 1, // offset row
      ]);
      final kernel = KernelInfo(width: 5, height: 5, values: matrix);
      await wand.magickColorMatrixImage(colorMatrix: kernel);
    }

    // 应用总曝光强度 (Multiply)即 totalExposure-1
    // totalExposure = (hdrIntensity * fineTuneBrightness) + 1
    await wand.magickEvaluateImage(
      operator: MagickEvaluateOperator.MultiplyEvaluateOperator,
      value: totalExposure - 1,
    );

    // 应用 Power 函数以调整对比度
    await wand.magickEvaluateImage(
      operator: MagickEvaluateOperator.PowEvaluateOperator,
      value: 0.9,
    );
  }

  /// ICC gain 后处理：转换回 sRGB 并嵌入 BT.2020 配置文件
  Future<void> _applyIccGainPostProcess(MagickWand wand) async {
    // 转换回 sRGB
    await wand.magickTransformImageColorspace(ColorspaceType.sRGBColorspace);
    // 嵌入 BT.2020 ICC 配置文件 (仅嵌入，不做 CMS 转换)
    if (_bt2020Profile != null) {
      wand.magickSetImageProfile('icc', _bt2020Profile!);
    }
  }

  @override
  Future<Uint8List> convertSdrToHdr({
    required Uint8List inputBytes,
    required ConversionSettings settings,
    void Function(double progress)? onProgress,
  }) async {
    await initialize();

    final wand = MagickWand.newMagickWand();
    try {
      // 读取输入图像
      final success = await wand.magickReadImageBlob(inputBytes);
      if (!success) {
        final ex = wand.magickGetException();
        throw Exception('读取图像失败: ${ex.description}');
      }

      // 应用 HDR 处理（线性 RGB 空间中的曝光和对比度调整）
      await _applyHdrProcessing(wand, settings);

      // 格式特定的后处理
      switch (settings.outputFormat) {
        case OutputFormat.hdrPng:
          // ICC gain：转 sRGB + 嵌入 BT.2020 配置文件（8-bit 足够）
          await _applyIccGainPostProcess(wand);
          await wand.magickSetImageDepth(8);
          wand.magickSetImageFormat('png');
          wand.magickSetImageCompression(CompressionType.ZipCompression);
          wand.magickSetOption('png:compression-level', '9');
          wand.magickSetOption('png:color-type', '2');
        case OutputFormat.avif:
          await _applyIccGainPostProcess(wand);
          await wand.magickSetImageDepth(12);
          wand.magickSetImageFormat('avif');
          wand.magickSetImageCompressionQuality(95);
          wand.magickSetOption('avif:lossless', 'false');
        case OutputFormat.ultraHdrJpeg:
          await _applyIccGainPostProcess(wand);
          await wand.magickSetImageDepth(8);
          wand.magickSetImageFormat('jpg');
          wand.magickSetImageCompressionQuality(98);
      }

      // 设置 HDR 元数据
      wand.magickSetImageProperty(
        'HDR:Exposure',
        settings.totalExposure.toStringAsFixed(3),
      );

      // 获取输出 blob
      final outputBlob = await wand.magickGetImageBlob();
      if (outputBlob == null) {
        throw Exception('生成输出图像失败');
      }

      return outputBlob;
    } finally {
      await wand.destroyMagickWand();
    }
  }

  @override
  Future<Uint8List> convertForPreview({
    required Uint8List inputBytes,
    required ConversionSettings settings,
    void Function(double progress)? onProgress,
  }) async {
    await initialize();

    final wand = MagickWand.newMagickWand();
    try {
      await wand.magickReadImageBlob(inputBytes);
      final width = wand.magickGetImageWidth();
      final height = wand.magickGetImageHeight();

      // 缩放预览到最大 800px
      const maxPreviewSize = 800;
      if (width > maxPreviewSize || height > maxPreviewSize) {
        double scale;
        if (width > height) {
          scale = maxPreviewSize / width;
        } else {
          scale = maxPreviewSize / height;
        }
        await wand.magickResizeImage(
          columns: (width * scale).round(),
          rows: (height * scale).round(),
          filter: FilterType.LanczosFilter,
        );
      }

      // 应用 HDR 处理
      await _applyHdrProcessing(wand, settings);

      // 预览统一用 ICC gain 后处理（8-bit PNG 保留 ICC 配置文件）
      await _applyIccGainPostProcess(wand);
      await wand.magickSetImageDepth(8);

      wand.magickSetImageFormat('png');
      wand.magickSetImageCompression(CompressionType.ZipCompression);
      wand.magickSetOption('png:compression-level', '6');
      wand.magickSetOption('png:color-type', '2');

      final outputBlob = await wand.magickGetImageBlob();
      if (outputBlob == null) {
        throw Exception('生成预览失败');
      }
      return outputBlob;
    } finally {
      await wand.destroyMagickWand();
    }
  }

  @override
  String getOutputExtension(ConversionSettings settings) {
    return settings.outputFormat.value;
  }

  @override
  void openHdrPreview(Uint8List pngBytes) {}

  @override
  void dismissHdrPreview() {}
}
