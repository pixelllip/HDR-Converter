import 'dart:async';
import 'dart:math' as math;
import 'dart:typed_data';
import 'package:flutter/services.dart' show rootBundle;
import 'package:image/image.dart' as img;
import 'package:web/web.dart' as web;
import '../models/conversion_settings.dart';
import 'hdr_converter_gpu.dart';
import 'hdr_converter_platform.dart';
import '../widgets/web/hdr_viewer.dart';

/// SDR 转 HDR 转换器 — Web 端实现
///
/// 使用纯 Dart [image] 包，不依赖任何原生代码。
class HdrConverter implements HdrConverterPlatform {
  static final HdrConverter _instance = HdrConverter._();

  /// 获取单例实例
  static HdrConverter get instance => _instance;

  HdrConverter._();

  bool _initialized = false;
  Uint8List? _bt2020Profile;

  /// GPU 加速是否可用 (Web 端不可用)
  bool get isGpuAvailable => false;

  /// GPU 后端名称
  String get gpuBackendName => 'None';

  /// GPU 初始化错误消息
  String? get gpuErrorMessage => null;

  /// 最近一次导出实际使用的后端
  String lastUsedBackend = 'CPU';

  Future<ProcessedImage?> processImageBytes(
    Uint8List inputBytes,
    ConversionSettings settings,
  ) async => null;

  @override
  Future<void> initialize() async {
    if (_initialized) return;
    _bt2020Profile = (await rootBundle.load(
      'assets/2020_profile.icc',
    )).buffer.asUint8List();
    _initialized = true;
  }

  // ===== 色彩空间转换 =====

  /// sRGB 伽马解码（浮点值 → 线性浮点）
  static double _srgbToLinearDouble(double d) {
    if (d <= 0.04045) return d / 12.92;
    return math.pow((d + 0.055) / 1.055, 2.4).toDouble();
  }

  /// 线性浮点 → sRGB 伽马编码
  static double _linearToSrgb(double v) {
    if (v <= 0.0031308) return v * 12.92;
    return 1.055 * math.pow(v, 1.0 / 2.4) - 0.055;
  }

  /// 将像素钳制到合法范围并转为 8-bit 整数
  static int _clampToUint8(double v) {
    final clamped = v.clamp(0.0, 1.0);
    return (clamped * 255.0).round();
  }

  // ===== 核心处理管线（分块异步） =====

  /// 预览批次：每批行数少 → 频繁 yield → UI 流畅
  static const int _batchRowsPreview = 30;

  /// 导出批次：每批行数多 → 减少 yield → 处理更快
  static const int _batchRowsExport = 150;

  /// 让出事件循环
  static Future<void> _yield() => Future.delayed(Duration.zero);

  /// 分块处理（预览用高频 yield，导出用低频 yield）
  static Future<void> _processRowsAsync({
    required int height,
    required int width,
    required void Function(int yStart, int yEnd) processRowBatch,
    void Function(double progress)? onProgress,
    double progressStart = 0.0,
    double progressEnd = 1.0,
    bool isExport = false,
  }) async {
    final batchSize = isExport ? _batchRowsExport : _batchRowsPreview;
    // 导出时每 5 批才 yield 一次，减少开销
    final yieldInterval = isExport ? 5 : 1;

    int batchCount = 0;
    for (int y = 0; y < height; y += batchSize) {
      final yEnd = (y + batchSize < height) ? y + batchSize : height;
      processRowBatch(y, yEnd);

      batchCount++;
      // 报告进度
      final p = progressStart + (progressEnd - progressStart) * (yEnd / height);
      onProgress?.call(p);

      // 按间隔 yield
      if (batchCount % yieldInterval == 0) {
        await _yield();
      }
    }
  }

  /// 在浮点缓冲区上应用 HDR 处理（分块异步）
  static Future<void> _processFloatBufferAsync({
    required Float64List buffer,
    required int width,
    required int height,
    required ConversionSettings settings,
    void Function(double progress)? onProgress,
    bool isExport = false,
  }) async {
    final totalExposure = settings.totalExposure - 1;
    final rAdj = settings.rgbAdjustment.red;
    final gAdj = settings.rgbAdjustment.green;
    final bAdj = settings.rgbAdjustment.blue;

    // === Pass 1: sRGB→线性 + 同时计算平均亮度（只需一次遍历）===
    double sum = 0.0;
    await _processRowsAsync(
      height: height,
      width: width,
      isExport: isExport,
      processRowBatch: (yStart, yEnd) {
        for (int y = yStart; y < yEnd; y++) {
          final rowStart = y * width * 3;
          final rowEnd = rowStart + width * 3;
          for (int i = rowStart; i < rowEnd; i += 3) {
            final r = _srgbToLinearDouble(buffer[i]);
            final g = _srgbToLinearDouble(buffer[i + 1]);
            final b = _srgbToLinearDouble(buffer[i + 2]);
            buffer[i] = r;
            buffer[i + 1] = g;
            buffer[i + 2] = b;
            // 累加亮度用于自动伽马
            sum += 0.2126 * r + 0.7152 * g + 0.0722 * b;
          }
        }
      },
      onProgress: onProgress,
      progressStart: 0.0,
      progressEnd: isExport ? 0.2 : 0.25,
    );

    // === Pass 2: 自动伽马（线性均值已算好）===
    final numPixels = width * height;
    final mean = sum / numPixels;
    if (mean > 0.001 && mean < 0.999) {
      final gamma = math.log(0.5) / math.log(mean);
      final clampedGamma = gamma.clamp(0.3, 3.0);
      await _processRowsAsync(
        height: height,
        width: width,
        isExport: isExport,
        processRowBatch: (yStart, yEnd) {
          final start = yStart * width * 3;
          final end = yEnd * width * 3;
          for (int i = start; i < end; i++) {
            buffer[i] = math
                .pow(buffer[i].clamp(0.0, double.maxFinite), clampedGamma)
                .toDouble();
          }
        },
        onProgress: onProgress,
        progressStart: isExport ? 0.2 : 0.25,
        progressEnd: isExport ? 0.3 : 0.35,
      );
    }

    // === Pass 3: RGB 调整 + 曝光 + Power + sRGB 编码 ===
    await _processRowsAsync(
      height: height,
      width: width,
      isExport: isExport,
      processRowBatch: (yStart, yEnd) {
        for (int y = yStart; y < yEnd; y++) {
          final rowStart = y * width * 3;
          final rowEnd = rowStart + width * 3;
          for (int i = rowStart; i < rowEnd; i += 3) {
            double r = buffer[i];
            double g = buffer[i + 1];
            double b = buffer[i + 2];

            r *= rAdj;
            g *= gAdj;
            b *= bAdj;

            r *= totalExposure;
            g *= totalExposure;
            b *= totalExposure;

            r = math
                .pow(r.clamp(0.0, double.maxFinite), settings.gamma)
                .toDouble();
            g = math
                .pow(g.clamp(0.0, double.maxFinite), settings.gamma)
                .toDouble();
            b = math
                .pow(b.clamp(0.0, double.maxFinite), settings.gamma)
                .toDouble();

            buffer[i] = _linearToSrgb(r);
            buffer[i + 1] = _linearToSrgb(g);
            buffer[i + 2] = _linearToSrgb(b);
          }
        }
      },
      onProgress: onProgress,
      progressStart: isExport ? 0.3 : 0.35,
      progressEnd: isExport ? 0.5 : 0.6,
    );
  }

  // ===== 公开 API =====

  @override
  Future<Uint8List> convertSdrToHdr({
    required Uint8List inputBytes,
    required ConversionSettings settings,
    void Function(double progress)? onProgress,
  }) async {
    await initialize();

    onProgress?.call(0.0);

    // 解码输入图像
    final original = img.decodeImage(inputBytes);
    if (original == null) {
      throw Exception('无法解码输入图像');
    }
    onProgress?.call(0.05);

    final w = original.width;
    final h = original.height;

    // 如果含 Alpha 通道，与黑色背景合并（分块）
    if (original.hasAlpha) {
      await _processRowsAsync(
        height: h,
        width: w,
        isExport: true,
        processRowBatch: (yStart, yEnd) {
          for (int y = yStart; y < yEnd; y++) {
            for (int x = 0; x < w; x++) {
              final pixel = original.getPixel(x, y);
              final a = pixel.a / 255.0;
              original.setPixelRgba(
                x,
                y,
                (pixel.r * a).round(),
                (pixel.g * a).round(),
                (pixel.b * a).round(),
                255,
              );
            }
          }
        },
        onProgress: onProgress,
        progressStart: 0.05,
        progressEnd: 0.1,
      );
    }

    final numPixels = w * h;

    // 提取像素到浮点缓冲区（分块，导出大批次）
    final buffer = Float64List(numPixels * 3);
    await _processRowsAsync(
      height: h,
      width: w,
      isExport: true,
      processRowBatch: (yStart, yEnd) {
        for (int y = yStart; y < yEnd; y++) {
          final rowStart = y * w * 3;
          for (int x = 0; x < w; x++) {
            final p = original.getPixel(x, y);
            final idx = rowStart + x * 3;
            buffer[idx] = p.r / 255.0;
            buffer[idx + 1] = p.g / 255.0;
            buffer[idx + 2] = p.b / 255.0;
          }
        }
      },
      onProgress: onProgress,
      progressStart: 0.1,
      progressEnd: 0.15,
    );

    // 应用 HDR 处理（分块异步，导出模式→大批次）
    await _processFloatBufferAsync(
      buffer: buffer,
      width: w,
      height: h,
      settings: settings,
      onProgress: onProgress,
      isExport: true,
    );

    // 创建 8-bit 输出图像（ICC gain 技术用 8-bit sRGB 值 + BT.2020 配置）
    final output = img.Image(width: w, height: h, numChannels: 3);
    if (_bt2020Profile != null) {
      output.iccProfile = img.IccProfile(
        'BT.2020',
        img.IccProfileCompression.none,
        _bt2020Profile!,
      );
    }
    onProgress?.call(0.65);

    // 写回像素到输出图像（分块）
    await _processRowsAsync(
      height: h,
      width: w,
      isExport: true,
      processRowBatch: (yStart, yEnd) {
        for (int y = yStart; y < yEnd; y++) {
          final rowStart = y * w * 3;
          for (int x = 0; x < w; x++) {
            final idx = rowStart + x * 3;
            output.setPixelRgba(
              x,
              y,
              _clampToUint8(buffer[idx]),
              _clampToUint8(buffer[idx + 1]),
              _clampToUint8(buffer[idx + 2]),
              255,
            );
          }
        }
      },
      onProgress: onProgress,
      progressStart: 0.65,
      progressEnd: 0.8,
    );

    // 按输出格式编码
    Uint8List result;
    switch (settings.outputFormat) {
      case OutputFormat.hdrPng:
        onProgress?.call(0.85);
        result = _encodeHdrPng(output);
        onProgress?.call(1.0);
        return result;
      case OutputFormat.ultraHdrJpeg:
        throw UnsupportedError('Ultra HDR JPEG 在 Web 端暂不支持，请使用 HDR PNG');
    }
  }

  /// 编码为 HDR PNG（8-bit + ICC 配置文件）
  Uint8List _encodeHdrPng(img.Image image) {
    // level=3 比 level=9 快 5-10 倍，文件体积略大但可接受
    final encoder = img.PngEncoder(level: 3);
    return encoder.encode(image);
  }

  @override
  Future<Uint8List> convertForPreview({
    required Uint8List inputBytes,
    required ConversionSettings settings,
    void Function(double progress)? onProgress,
  }) async {
    await initialize();

    onProgress?.call(0.0);

    // 解码输入图像
    final original = img.decodeImage(inputBytes);
    if (original == null) {
      throw Exception('无法解码输入图像');
    }
    onProgress?.call(0.05);

    // 缩放预览到最大 800px
    img.Image working = original;
    const maxPreviewSize = 800;
    if (working.width > maxPreviewSize || working.height > maxPreviewSize) {
      double scale;
      if (working.width > working.height) {
        scale = maxPreviewSize / working.width;
      } else {
        scale = maxPreviewSize / working.height;
      }
      working = img.copyResize(
        working,
        width: (working.width * scale).round(),
        height: (working.height * scale).round(),
        interpolation: img.Interpolation.average,
      );
    }
    onProgress?.call(0.1);

    final w = working.width;
    final h = working.height;
    final numPixels = w * h;

    // 提取像素到浮点缓冲区（分块）
    final buffer = Float64List(numPixels * 3);
    await _processRowsAsync(
      height: h,
      width: w,
      processRowBatch: (yStart, yEnd) {
        for (int y = yStart; y < yEnd; y++) {
          final rowStart = y * w * 3;
          for (int x = 0; x < w; x++) {
            final p = working.getPixel(x, y);
            final idx = rowStart + x * 3;
            buffer[idx] = p.r / 255.0;
            buffer[idx + 1] = p.g / 255.0;
            buffer[idx + 2] = p.b / 255.0;
          }
        }
      },
      onProgress: onProgress,
      progressStart: 0.1,
      progressEnd: 0.2,
    );

    // 应用 HDR 处理（分块异步，预览模式→小批次）
    await _processFloatBufferAsync(
      buffer: buffer,
      width: w,
      height: h,
      settings: settings,
      onProgress: onProgress,
    );

    // 创建 8-bit 预览图像（屏幕显示无需 16-bit）
    final preview = img.Image(width: w, height: h, numChannels: 3);

    // 写回像素（分块）
    await _processRowsAsync(
      height: h,
      width: w,
      processRowBatch: (yStart, yEnd) {
        for (int y = yStart; y < yEnd; y++) {
          final rowStart = y * w * 3;
          for (int x = 0; x < w; x++) {
            final idx = rowStart + x * 3;
            preview.setPixelRgba(
              x,
              y,
              _clampToUint8(buffer[idx]),
              _clampToUint8(buffer[idx + 1]),
              _clampToUint8(buffer[idx + 2]),
              255,
            );
          }
        }
      },
      onProgress: onProgress,
      progressStart: 0.6,
      progressEnd: 0.8,
    );

    // 嵌入 BT.2020 ICC 配置 + 编码为 16-bit PNG
    if (_bt2020Profile != null) {
      preview.iccProfile = img.IccProfile(
        'BT.2020',
        img.IccProfileCompression.none,
        _bt2020Profile!,
      );
    }
    onProgress?.call(0.85);
    final pngEncoder = img.PngEncoder(level: 6);
    final result = pngEncoder.encode(preview);
    onProgress?.call(1.0);
    return result;
  }

  @override
  String getOutputExtension(ConversionSettings settings) {
    return settings.outputFormat.value;
  }

  @override
  void openHdrPreview(Uint8List pngBytes) {
    showHdrPreviewOverlay(pngBytes);
  }

  @override
  void dismissHdrPreview() {
    _dismissOverlay();
  }
}

/// 关闭覆盖层（调用 hdr_viewer 的函数）
void _dismissOverlay() {
  // 通过 JS interop 移除覆盖层 DOM 元素
  final existing = web.document.getElementById('hdr_overlay');
  if (existing != null) existing.remove();
}
