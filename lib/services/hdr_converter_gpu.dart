import 'dart:async';
import 'dart:ffi';
import 'dart:isolate';
import 'dart:typed_data';
import 'dart:ui' show instantiateImageCodec, ImageByteFormat;
import 'package:ffi/ffi.dart';
import 'package:image/image.dart' as img;
import '../models/conversion_settings.dart';
import 'hdr_gpu_ffi.dart';

/// GPU 处理后的图像数据 (不含编码)
class ProcessedImage {
  final Uint8List rgba;
  final int width;
  final int height;
  ProcessedImage({required this.rgba, required this.width, required this.height});
}

// ===== 持久化 GPU 工作 Isolate =====
/// 发送给 GPU 工作者的请求
/// GPU 工作者 Isolate 入口 (持久运行, 只初始化一次 CUDA)
void _gpuWorkerEntry(SendPort mainPort) {
  final receivePort = ReceivePort();

  DynamicLibrary? lib;
  int Function(int)? initFn;
  int Function(Pointer<Uint8>, int, int, Pointer<Uint8>, double, double, double, double, double)? processFn;
  Pointer<Utf8> Function()? errorFn;

  try {
    lib = DynamicLibrary.open('hdr_gpu.dll');
    initFn = lib.lookupFunction<Int32 Function(Int32), int Function(int)>('hdr_gpu_init');
    processFn = lib.lookupFunction<
        Int32 Function(Pointer<Uint8>, Int32, Int32, Pointer<Uint8>, Float, Float, Float, Float, Float),
        int Function(Pointer<Uint8>, int, int, Pointer<Uint8>, double, double, double, double, double)
      >('hdr_gpu_process');
    errorFn = lib.lookupFunction<Pointer<Utf8> Function(), Pointer<Utf8> Function()>('hdr_gpu_error');
    if (initFn(0) != 0) {
      final errMsg = errorFn() != nullptr ? errorFn().toDartString() : 'init failed';
      mainPort.send([null, errMsg]);
      return;
    }
  } catch (e) {
    mainPort.send([null, 'Worker DLL error: $e']);
    return;
  }

  // 一次性发送 SendPort + 初始化结果 (避免 ReceivePort 重复订阅)
  mainPort.send([receivePort.sendPort, true]);

  receivePort.listen((msg) {
    try {
      final replyPort = msg[0] as SendPort;
      final rgba = msg[1] as Uint8List;
      final w = msg[2] as int, h = msg[3] as int;
      final te = msg[4] as double, g = msg[5] as double;
      final rA = msg[6] as double, gA = msg[7] as double, bA = msg[8] as double;

      final out = Uint8List(w * h * 4);
      final inPtr = calloc<Uint8>(rgba.length);
      final outPtr = calloc<Uint8>(out.length);
      inPtr.asTypedList(rgba.length).setAll(0, rgba);

      final ret = processFn!(inPtr, w, h, outPtr, te, g, rA, gA, bA);
      if (ret == 0) {
        for (int i = 0; i < out.length; i++) {
          out[i] = outPtr.asTypedList(out.length)[i];
        }
      }
      calloc.free(inPtr); calloc.free(outPtr);
      if (ret == 0) {
        replyPort.send([out, null]);
      } else {
        final errMsg = errorFn!() != nullptr ? errorFn().toDartString() : 'process error $ret';
        replyPort.send([null, errMsg]);
      }
    } catch (e) {
      (msg[0] as SendPort).send([null, 'Worker exception: $e']);
    }
  });
}

/// GPU 加速的 HDR 转换器
class GpuAcceleratedConverter {
  final HdrGpuEngine _engine = HdrGpuEngine.instance;
  bool _gpuAvailable = false;
  SendPort? _gpuWorkerPort;

  static Future<void> yieldNow() => Future<void>.delayed(Duration.zero);

  /// 按行分批处理, 每 [batchRows] 行 yield 一次, 避免 UI 卡死
  static Future<void> processRowsAsync({
    required int height,
    required int width,
    required void Function(int yStart, int yEnd) processBatch,
    int batchRows = 40,
  }) async {
    for (int y = 0; y < height; y += batchRows) {
      final yEnd = (y + batchRows < height) ? y + batchRows : height;
      processBatch(y, yEnd);
      await yieldNow();
    }
  }

  /// 上一次初始化/处理的错误消息
  String? _lastErrorMessage;

  Future<bool> tryInitialize() async {
    try {
      _gpuAvailable = await _engine.initialize();
      if (!_gpuAvailable) {
        _lastErrorMessage = _engine.lastError ?? 'hdr_gpu.dll 初始化返回失败';
        return false;
      }
      // 启动持久 GPU 工作 Isolate
      try {
        final mainPort = ReceivePort();
        await Isolate.spawn(_gpuWorkerEntry, mainPort.sendPort);
        final msg = await mainPort.first;
        if (msg is List && msg.length == 2) {
          _gpuWorkerPort = msg[0] as SendPort;
          final ok = msg[1];
          if (ok != true) {
            _gpuWorkerPort = null;
            _gpuAvailable = false;
            _lastErrorMessage = 'GPU 工作 Isolate 初始化失败';
          }
        } else if (msg == null) {
          _gpuAvailable = false;
          _lastErrorMessage = 'GPU 工作 Isolate 初始化返回 null';
        } else {
          _gpuAvailable = false;
          _lastErrorMessage = 'GPU 工作 Isolate 返回异常类型: ${msg.runtimeType}';
        }
      } catch (e) {
        _gpuAvailable = false;
        _lastErrorMessage = '启动 GPU 工作 Isolate 异常: $e';
      }
      return _gpuAvailable;
    } catch (e) {
      _gpuAvailable = false;
      _lastErrorMessage = 'GPU 初始化异常: $e';
      return false;
    }
  }

  bool get isGpuAvailable => _gpuAvailable;
  String get backendName => _engine.backendName;
  String? get errorMessage => _lastErrorMessage ?? _engine.lastError;

  /// 异步 GPU 处理 (通过持久 Isolate, 不阻塞主线程)
  Future<Uint8List?> processOnGpuAsync(
    Uint8List rgbaBytes, {
    required int width,
    required int height,
    required ConversionSettings settings,
  }) async {
    if (!_gpuAvailable || _gpuWorkerPort == null) return null;
    final totalExposure = settings.totalExposure - 1;
    final replyPort = ReceivePort();
    _gpuWorkerPort!.send([
      replyPort.sendPort, rgbaBytes, width, height,
      totalExposure, settings.gamma,
      settings.rgbAdjustment.red, settings.rgbAdjustment.green,
      settings.rgbAdjustment.blue,
    ]);
    final v = await replyPort.first;
    if (v is List && v.length == 2) {
      _lastErrorMessage = v[1] as String?;
      return v[0] as Uint8List?;
    }
    return v as Uint8List?;
  }

  /// Flutter 引擎异步解码 → GPU 处理 → RGBA (全程不阻塞主线程)
  Future<ProcessedImage?> processImageBytes(
    Uint8List inputBytes,
    ConversionSettings settings,
  ) async {
    if (!_gpuAvailable) return null;
    await yieldNow();
    // Flutter 引擎解码 (异步, 引擎线程执行, 主线程不阻塞)
    final codec = await instantiateImageCodec(inputBytes);
    final frame = await codec.getNextFrame();
    final uiImage = frame.image;
    final w = uiImage.width, h = uiImage.height;
    final byteData = await uiImage.toByteData(format: ImageByteFormat.rawRgba);
    uiImage.dispose();
    codec.dispose();
    if (byteData == null) return null;

    // rawRgba 含 alpha, 与黑色合并
    final src = byteData.buffer.asUint8List();
    final rgba = Uint8List(w * h * 4);
    for (int i = 0; i < w * h; i++) {
      final si = i * 4;
      final a = src[si + 3];
      if (a == 255) {
        rgba[si] = src[si]; rgba[si + 1] = src[si + 1];
        rgba[si + 2] = src[si + 2]; rgba[si + 3] = 255;
      } else if (a == 0) {
        rgba[si] = 0; rgba[si + 1] = 0; rgba[si + 2] = 0; rgba[si + 3] = 255;
      } else {
        rgba[si] = (src[si] * a / 255).round();
        rgba[si + 1] = (src[si + 1] * a / 255).round();
        rgba[si + 2] = (src[si + 2] * a / 255).round();
        rgba[si + 3] = 255;
      }
    }

    // GPU 处理 (后台 Isolate, 不阻塞主线程)
    final resultRgba = await processOnGpuAsync(rgba, width: w, height: h, settings: settings);
    if (resultRgba == null) return null;
    return ProcessedImage(rgba: resultRgba, width: w, height: h);
  }

  Future<Uint8List?> convertWithGpu({
    required Uint8List inputBytes,
    required ConversionSettings settings,
  }) async {
    if (!_gpuAvailable) return null;
    await yieldNow();
    final original = img.decodeImage(inputBytes);
    if (original == null) return null;
    final w = original.width, h = original.height;

    if (original.hasAlpha) {
      await processRowsAsync(height: h, width: w, processBatch: (yStart, yEnd) {
        for (int y = yStart; y < yEnd; y++) {
          for (int x = 0; x < w; x++) {
            final p = original.getPixel(x, y);
            final a = p.a / 255.0;
            original.setPixelRgba(x, y, (p.r * a).round(), (p.g * a).round(), (p.b * a).round(), 255);
          }
        }
      });
    } else {
      await yieldNow();
    }

    final rgbaBytes = Uint8List(w * h * 4);
    await processRowsAsync(height: h, width: w, processBatch: (yStart, yEnd) {
      for (int y = yStart; y < yEnd; y++) {
        final rs = y * w * 4;
        for (int x = 0; x < w; x++) {
          final p = original.getPixel(x, y);
          final i = rs + x * 4;
          rgbaBytes[i] = p.r.toInt(); rgbaBytes[i + 1] = p.g.toInt();
          rgbaBytes[i + 2] = p.b.toInt(); rgbaBytes[i + 3] = 255;
        }
      }
    });

    final resultRgba = await processOnGpuAsync(rgbaBytes, width: w, height: h, settings: settings);
    if (resultRgba == null) return null;

    final output = img.Image(width: w, height: h, numChannels: 3);
    await processRowsAsync(height: h, width: w, processBatch: (yStart, yEnd) {
      for (int y = yStart; y < yEnd; y++) {
        final rs = y * w * 4;
        for (int x = 0; x < w; x++) {
          final i = rs + x * 4;
          output.setPixelRgba(x, y, resultRgba[i], resultRgba[i + 1], resultRgba[i + 2], 255);
        }
      }
    });
    return null;
  }

  Future<Uint8List?> convertAndEncode({
    required Uint8List inputBytes,
    required ConversionSettings settings,
    Uint8List? iccProfile,
  }) async {
    if (!_gpuAvailable) return null;
    await yieldNow(); // 先让出 UI, 再开始 CPU 重活
    final original = img.decodeImage(inputBytes);
    if (original == null) return null;
    final w = original.width, h = original.height;

    // Alpha 合并 (分批 yield)
    if (original.hasAlpha) {
      await processRowsAsync(height: h, width: w, processBatch: (yStart, yEnd) {
        for (int y = yStart; y < yEnd; y++) {
          for (int x = 0; x < w; x++) {
            final p = original.getPixel(x, y);
            final a = p.a / 255.0;
            original.setPixelRgba(x, y, (p.r * a).round(), (p.g * a).round(), (p.b * a).round(), 255);
          }
        }
      });
    } else {
      await yieldNow();
    }

    // 提取 RGBA (分批 yield)
    final rgbaBytes = Uint8List(w * h * 4);
    await processRowsAsync(height: h, width: w, processBatch: (yStart, yEnd) {
      for (int y = yStart; y < yEnd; y++) {
        final rs = y * w * 4;
        for (int x = 0; x < w; x++) {
          final p = original.getPixel(x, y);
          final i = rs + x * 4;
          rgbaBytes[i] = p.r.toInt(); rgbaBytes[i + 1] = p.g.toInt();
          rgbaBytes[i + 2] = p.b.toInt(); rgbaBytes[i + 3] = 255;
        }
      }
    });

    // GPU 处理 (后台 Isolate, 不阻塞)
    final resultRgba = await processOnGpuAsync(rgbaBytes, width: w, height: h, settings: settings);
    if (resultRgba == null) return null;

    // 创建输出图像 (分批 yield)
    final output = img.Image(width: w, height: h, numChannels: 3);
    await processRowsAsync(height: h, width: w, processBatch: (yStart, yEnd) {
      for (int y = yStart; y < yEnd; y++) {
        final rs = y * w * 4;
        for (int x = 0; x < w; x++) {
          final i = rs + x * 4;
          output.setPixelRgba(x, y, resultRgba[i], resultRgba[i + 1], resultRgba[i + 2], 255);
        }
      }
    });

    if (iccProfile != null) {
      output.iccProfile = img.IccProfile('BT.2020', img.IccProfileCompression.none, iccProfile);
    }

    switch (settings.outputFormat) {
      case OutputFormat.hdrPng:
        return img.PngEncoder(level: 3).encode(output);
      case OutputFormat.ultraHdrJpeg:
        final savedIcc = output.iccProfile;
        output.iccProfile = null;
        var jpegBytes = img.JpegEncoder(quality: 98).encode(output);
        if (savedIcc != null) {
          jpegBytes = _injectIccIntoJpeg(jpegBytes, savedIcc.decompressed());
        }
        return jpegBytes;
    }
  }

  static Uint8List _injectIccIntoJpeg(Uint8List jpegBytes, Uint8List iccData) {
    final chunkSize = 18 + iccData.length;
    final iccChunk = ByteData(chunkSize)
      ..setUint16(0, 0xFFE2)
      ..setUint16(2, 16 + iccData.length)
      ..setUint32(4, 0x4943435F)
      ..setUint32(8, 0x50524F46)
      ..setUint32(12, 0x494C4500)
      ..setUint8(16, 1)..setUint8(17, 1);
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

  void dispose() {
    if (_gpuAvailable) { _engine.cleanup(); _gpuAvailable = false; }
  }
}
