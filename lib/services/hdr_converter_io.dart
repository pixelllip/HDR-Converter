import 'dart:async';
import 'dart:math' as math;
import 'dart:typed_data';
import 'package:flutter/services.dart' show rootBundle;
import 'package:image/image.dart' as img;
import '../models/conversion_settings.dart';
import 'hdr_converter_platform.dart';
import 'hdr_converter_gpu.dart';

/// SDR 转 HDR 转换器 — 桌面端实现
///
/// 使用纯 Dart [image] 包，不依赖任何原生代码。
/// 如果 GPU 加速可用 (DirectCompute / CUDA)，自动使用 GPU 加速像素处理。
class HdrConverter implements HdrConverterPlatform {
  static final HdrConverter _instance = HdrConverter._();

  /// 获取单例实例
  static HdrConverter get instance => _instance;

  HdrConverter._();

  bool _initialized = false;
  Uint8List? _bt2020Profile;
  GpuAcceleratedConverter? _gpuConverter;
  bool _gpuAttempted = false;
  Future<void>? _initFuture; // 防止并发重入

  @override
  Future<void> initialize() async {
    if (_initialized) return;
    // 如果正在初始化中, 等待完成
    if (_initFuture != null) return _initFuture!;

    _initFuture = _doInitialize();
    return _initFuture!;
  }

  Future<void> _doInitialize() async {
    _bt2020Profile = (await rootBundle.load(
      'assets/2020_profile.icc',
    )).buffer.asUint8List();

    // 尝试初始化 GPU 加速引擎 (只尝试一次)
    if (!_gpuAttempted) {
      _gpuAttempted = true;
      try {
        _gpuConverter = GpuAcceleratedConverter();
        final gpuOk = await _gpuConverter!.tryInitialize();
        if (!gpuOk) {
          // 保留 _gpuConverter 以便查询错误信息
        }
      } catch (_) {
        _gpuConverter = null;
      }
    }

    _initialized = true;
  }

  /// GPU 加速是否可用
  bool get isGpuAvailable => _gpuConverter?.isGpuAvailable ?? false;

  /// GPU 后端名称 (如 "CUDA" 或 "DirectCompute")
  String get gpuBackendName => _gpuConverter?.backendName ?? 'None';

  /// GPU 初始化错误消息 (仅在 GPU 不可用时有意义)
  String? get gpuErrorMessage => _gpuConverter?.errorMessage;

  /// 最近一次导出实际使用的后端 (GPU:xxx / CPU)
  String lastUsedBackend = 'CPU';

  /// GPU 处理 → 返回 RGBA (编码由 Isolate 完成)
  Future<ProcessedImage?> processImageBytes(
    Uint8List inputBytes,
    ConversionSettings settings,
  ) {
    return _gpuConverter?.processImageBytes(inputBytes, settings) ?? Future.value(null);
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

  /// 让出事件循环
  static Future<void> _yield() => Future.delayed(Duration.zero);

  /// 预览批次大小
  static const int _batchRowsPreview = 30;

  /// 分块处理（批量 yield 保持 UI 响应）
  static Future<void> _processRowsAsync({
    required int height,
    required int width,
    required void Function(int yStart, int yEnd) processRowBatch,
    void Function(double progress)? onProgress,
    double progressStart = 0.0,
    double progressEnd = 1.0,
  }) async {
    for (int y = 0; y < height; y += _batchRowsPreview) {
      final yEnd = (y + _batchRowsPreview < height)
          ? y + _batchRowsPreview
          : height;
      processRowBatch(y, yEnd);
      final p = progressStart + (progressEnd - progressStart) * (yEnd / height);
      onProgress?.call(p);
      await _yield();
    }
  }

  /// 在浮点缓冲区上应用 HDR 处理（分块异步，预览用）
  static Future<void> _processFloatBufferAsync({
    required Float64List buffer,
    required int width,
    required int height,
    required ConversionSettings settings,
    void Function(double progress)? onProgress,
    double progressStart = 0.0,
    double progressEnd = 1.0,
  }) async {
    final totalExposure = settings.totalExposure - 1;
    final rAdj = settings.rgbAdjustment.red;
    final gAdj = settings.rgbAdjustment.green;
    final bAdj = settings.rgbAdjustment.blue;

    // === Pass 1: sRGB→线性 + 计算平均亮度 ===
    double sum = 0.0;
    await _processRowsAsync(
      height: height,
      width: width,
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
            sum += 0.2126 * r + 0.7152 * g + 0.0722 * b;
          }
        }
      },
      onProgress: onProgress,
      progressStart: progressStart,
      progressEnd: progressStart + (progressEnd - progressStart) * 0.4,
    );

    // === Pass 2: 自动伽马 ===
    final numPixels = width * height;
    final mean = sum / numPixels;
    if (mean > 0.001 && mean < 0.999) {
      final gamma = math.log(0.5) / math.log(mean);
      final clampedGamma = gamma.clamp(0.3, 3.0);
      await _processRowsAsync(
        height: height,
        width: width,
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
        progressStart: progressStart + (progressEnd - progressStart) * 0.4,
        progressEnd: progressStart + (progressEnd - progressStart) * 0.55,
      );
    }

    // === Pass 3: RGB 调整 + 曝光 + 伽马 + sRGB 编码 ===
    await _processRowsAsync(
      height: height,
      width: width,
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
      progressStart: progressStart + (progressEnd - progressStart) * 0.55,
      progressEnd: progressEnd,
    );
  }

  /// 同步处理（导出用，无 yield 开销）
  static void _processFloatBufferSync({
    required Float64List buffer,
    required int width,
    required int height,
    required ConversionSettings settings,
  }) {
    final totalExposure = settings.totalExposure - 1;
    final rAdj = settings.rgbAdjustment.red;
    final gAdj = settings.rgbAdjustment.green;
    final bAdj = settings.rgbAdjustment.blue;
    final numPixels = width * height;

    // Pass 1: sRGB→线性 + 计算平均亮度
    double sum = 0.0;
    for (int i = 0; i < numPixels * 3; i += 3) {
      final r = _srgbToLinearDouble(buffer[i]);
      final g = _srgbToLinearDouble(buffer[i + 1]);
      final b = _srgbToLinearDouble(buffer[i + 2]);
      buffer[i] = r;
      buffer[i + 1] = g;
      buffer[i + 2] = b;
      sum += 0.2126 * r + 0.7152 * g + 0.0722 * b;
    }

    // Pass 2: 自动伽马
    final mean = sum / numPixels;
    if (mean > 0.001 && mean < 0.999) {
      final gamma = math.log(0.5) / math.log(mean);
      final clampedGamma = gamma.clamp(0.3, 3.0);
      for (int i = 0; i < numPixels * 3; i++) {
        buffer[i] = math
            .pow(buffer[i].clamp(0.0, double.maxFinite), clampedGamma)
            .toDouble();
      }
    }

    // Pass 3: RGB 调整 + 曝光 + 伽马 + sRGB 编码
    for (int i = 0; i < numPixels * 3; i += 3) {
      double r = buffer[i];
      double g = buffer[i + 1];
      double b = buffer[i + 2];

      r *= rAdj;
      g *= gAdj;
      b *= bAdj;

      r *= totalExposure;
      g *= totalExposure;
      b *= totalExposure;

      r = math.pow(r.clamp(0.0, double.maxFinite), settings.gamma).toDouble();
      g = math.pow(g.clamp(0.0, double.maxFinite), settings.gamma).toDouble();
      b = math.pow(b.clamp(0.0, double.maxFinite), settings.gamma).toDouble();

      buffer[i] = _linearToSrgb(r);
      buffer[i + 1] = _linearToSrgb(g);
      buffer[i + 2] = _linearToSrgb(b);
    }
  }

  /// 向 JPEG 字节流注入 ICC APP2 标记
  static Uint8List _injectIccIntoJpeg(Uint8List jpegBytes, Uint8List iccData) {
    final chunkSize = 18 + iccData.length;
    final lengthValue = 16 + iccData.length;
    final iccChunk = ByteData(chunkSize)
      ..setUint16(0, 0xFFE2)
      ..setUint16(2, lengthValue)
      ..setUint32(4, 0x4943435F)
      ..setUint32(8, 0x50524F46)
      ..setUint32(12, 0x494C4500)
      ..setUint8(16, 1)
      ..setUint8(17, 1);
    for (int i = 0; i < iccData.length; i++) {
      iccChunk.setUint8(18 + i, iccData[i]);
    }
    final iccBytes = iccChunk.buffer.asUint8List();
    final result = Uint8List(jpegBytes.length + chunkSize);
    result.setRange(0, 2, jpegBytes.sublist(0, 2));
    result.setRange(2, 2 + chunkSize, iccBytes);
    result.setRange(2 + chunkSize, result.length, jpegBytes.sublist(2));
    return result;
  }

  /// 编码为 HDR PNG（8-bit + ICC 配置文件）
  Uint8List _encodeHdrPng(img.Image image) {
    final encoder = img.PngEncoder(level: 3);
    return encoder.encode(image);
  }

  @override
  Future<Uint8List> convertSdrToHdr({
    required Uint8List inputBytes,
    required ConversionSettings settings,
    void Function(double progress)? onProgress,
  }) async {
    await initialize();

    onProgress?.call(0.0);

    // === 优先 GPU 路径 (跳过 CPU 解码, 避免重复) ===
    if (_gpuConverter != null && _gpuConverter!.isGpuAvailable) {
      onProgress?.call(0.05);
      await Future<void>.delayed(const Duration(milliseconds: 1));
      final gpuResult = await _gpuConverter!.convertAndEncode(
        inputBytes: inputBytes,
        settings: settings,
        iccProfile: _bt2020Profile,
      );
      if (gpuResult != null) {
        lastUsedBackend = 'GPU: ${_gpuConverter!.backendName}';
        onProgress?.call(1.0);
        return gpuResult;
      }
      lastUsedBackend = 'CPU (GPU失败回退)';
      throw Exception('GPU processing failed'); // 触发 _exportHdr 的 catch → 走 Isolate 路径
    }

    // === CPU 路径 (GPU 不可用时) ===
    await Future<void>.delayed(const Duration(milliseconds: 1));
    lastUsedBackend = 'CPU';
    final original = img.decodeImage(inputBytes);
    if (original == null) throw Exception('无法解码输入图像');
    onProgress?.call(0.05);

    final w = original.width;
    final h = original.height;

    if (original.hasAlpha) {
      for (int y = 0; y < h; y++) {
        for (int x = 0; x < w; x++) {
          final pixel = original.getPixel(x, y);
          final a = pixel.a / 255.0;
          original.setPixelRgba(x, y,
            (pixel.r * a).round(), (pixel.g * a).round(), (pixel.b * a).round(), 255);
        }
      }
    }
    onProgress?.call(0.1);

    final numPixels = w * h;

    final buffer = Float64List(numPixels * 3);
    for (int y = 0; y < h; y++) {
      final rowStart = y * w * 3;
      for (int x = 0; x < w; x++) {
        final p = original.getPixel(x, y);
        final idx = rowStart + x * 3;
        buffer[idx] = p.r / 255.0;
        buffer[idx + 1] = p.g / 255.0;
        buffer[idx + 2] = p.b / 255.0;
      }
    }
    onProgress?.call(0.15);

    // 应用 HDR 处理（同步，无 yield 开销）
    _processFloatBufferSync(
      buffer: buffer,
      width: w,
      height: h,
      settings: settings,
    );
    onProgress?.call(0.6);

    // 创建 8-bit 输出图像
    final output = img.Image(width: w, height: h, numChannels: 3);
    if (_bt2020Profile != null) {
      output.iccProfile = img.IccProfile(
        'BT.2020',
        img.IccProfileCompression.none,
        _bt2020Profile!,
      );
    }

    // 写回像素到输出图像
    for (int y = 0; y < h; y++) {
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
    onProgress?.call(0.8);

    // 按输出格式编码
    Uint8List result;
    switch (settings.outputFormat) {
      case OutputFormat.hdrPng:
        result = _encodeHdrPng(output);
        onProgress?.call(1.0);
        return result;
      case OutputFormat.ultraHdrJpeg:
        final savedIcc = output.iccProfile;
        output.iccProfile = null;
        final encoder = img.JpegEncoder(quality: 98);
        var jpegBytes = encoder.encode(output);
        if (savedIcc != null) {
          jpegBytes = _injectIccIntoJpeg(jpegBytes, savedIcc.decompressed());
        }
        result = jpegBytes;
        onProgress?.call(1.0);
        return result;
    }
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

    // 提取像素到浮点缓冲区（分块异步）
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
      progressEnd: 0.15,
    );

    // 应用 HDR 处理（分块异步，保持 UI 响应）
    await _processFloatBufferAsync(
      buffer: buffer,
      width: w,
      height: h,
      settings: settings,
      onProgress: onProgress,
      progressStart: 0.15,
      progressEnd: 0.7,
    );

    // 创建预览图像
    final output = img.Image(width: w, height: h, numChannels: 3);
    if (_bt2020Profile != null) {
      output.iccProfile = img.IccProfile(
        'BT.2020',
        img.IccProfileCompression.none,
        _bt2020Profile!,
      );
    }

    // 写回像素（分块异步）
    await _processRowsAsync(
      height: h,
      width: w,
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
      progressStart: 0.7,
      progressEnd: 0.85,
    );

    final result = _encodeHdrPng(output);
    onProgress?.call(1.0);
    return result;
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
