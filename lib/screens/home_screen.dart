import 'dart:async';
import 'dart:isolate';
import 'dart:math' as math;
import 'dart:io' show Directory, File, FileSystemEntity, Platform;
import 'dart:typed_data';
import 'package:flutter/foundation.dart' show kIsWeb;
import 'package:flutter/material.dart';
import 'package:flutter/services.dart' show rootBundle;
import 'package:file_selector/file_selector.dart';
import 'package:archive/archive_io.dart';
import 'package:image/image.dart' as img;
import 'package:path/path.dart' as p;
import '../models/conversion_settings.dart';
import '../services/hdr_converter.dart';
import '../services/hdr_converter_gpu.dart';
import '../services/file_save_helper.dart';
import '../widgets/settings_panel.dart';

/// 批量文件信息
class _BatchFileInfo {
  final String name;
  final String baseName;
  final Uint8List bytes;
  Uint8List? previewBytes;
  Uint8List? outputBytes;
  bool isProcessed = false;
  String? error;

  _BatchFileInfo({
    required this.name,
    required this.baseName,
    required this.bytes,
  });
}

// =====================================================================
// Isolate 并行处理 — 每个 isolate 独立处理一张图片
// =====================================================================

/// 发送给 isolate 的任务参数
class _BatchTask {
  final int index;
  final Uint8List inputBytes;
  final double totalExposure;
  final double gamma;
  final double rAdj;
  final double gAdj;
  final double bAdj;
  final OutputFormat outputFormat;
  final Uint8List? iccProfile;

  _BatchTask({
    required this.index,
    required this.inputBytes,
    required this.totalExposure,
    required this.gamma,
    required this.rAdj,
    required this.gAdj,
    required this.bAdj,
    required this.outputFormat,
    this.iccProfile,
  });
}

/// isolate 返回的处理结果
class _BatchResult {
  final int index;
  final Uint8List? outputBytes;
  final String? error;
  _BatchResult({required this.index, this.outputBytes, this.error});
}

/// 在 Isolate 中执行 CPU HDR 处理 (不含 GPU)
Future<_BatchResult> _executeBatchTask(_BatchTask task) async {
  return Isolate.run(() {
    try {
      final original = img.decodeImage(task.inputBytes);
      if (original == null) return _BatchResult(index: task.index, error: '无法解码图片');
      if (original.hasAlpha) {
        for (int y = 0; y < original.height; y++) {
          for (int x = 0; x < original.width; x++) {
            final pxl = original.getPixel(x, y);
            final a = pxl.a / 255.0;
            original.setPixelRgba(x, y,
              (pxl.r * a).round(), (pxl.g * a).round(), (pxl.b * a).round(), 255);
          }
        }
      }
      final w = original.width, h = original.height, numPixels = w * h;
      final buffer = Float64List(numPixels * 3);
      for (int y = 0; y < h; y++) {
        final rs = y * w * 3;
        for (int x = 0; x < w; x++) {
          final pxl = original.getPixel(x, y);
          final i = rs + x * 3;
          buffer[i] = pxl.r / 255.0; buffer[i + 1] = pxl.g / 255.0; buffer[i + 2] = pxl.b / 255.0;
        }
      }
      _processBatchBuffer(buffer, w, h, task.totalExposure, task.gamma, task.rAdj, task.gAdj, task.bAdj);
      final output = img.Image(width: w, height: h, numChannels: 3);
      if (task.iccProfile != null) {
        output.iccProfile = img.IccProfile('BT.2020', img.IccProfileCompression.none, task.iccProfile!);
      }
      for (int y = 0; y < h; y++) {
        final rs = y * w * 3;
        for (int x = 0; x < w; x++) {
          final i = rs + x * 3;
          output.setPixelRgba(x, y, _batchClampUint8(buffer[i]), _batchClampUint8(buffer[i + 1]), _batchClampUint8(buffer[i + 2]), 255);
        }
      }
      Uint8List result;
      if (task.outputFormat == OutputFormat.hdrPng) {
        result = img.PngEncoder(level: 3).encode(output);
      } else {
        final savedIcc = output.iccProfile;
        output.iccProfile = null;
        var jpegBytes = img.JpegEncoder(quality: 98).encode(output);
        if (savedIcc != null) jpegBytes = _batchInjectIcc(jpegBytes, savedIcc.decompressed());
        result = jpegBytes;
      }
      return _BatchResult(index: task.index, outputBytes: result);
    } catch (e) {
      return _BatchResult(index: task.index, error: e.toString());
    }
  });
}

/// sRGB → 线性
double _batchSrgbToLinear(double d) {
  if (d <= 0.04045) return d / 12.92;
  return math.pow((d + 0.055) / 1.055, 2.4).toDouble();
}

/// 线性 → sRGB
double _batchLinearToSrgb(double v) {
  if (v <= 0.0031308) return v * 12.92;
  return 1.055 * math.pow(v, 1.0 / 2.4) - 0.055;
}

/// 钳制到 0-1 并转为 8-bit
int _batchClampUint8(double v) {
  return (v.clamp(0.0, 1.0) * 255.0).round();
}

/// 浮点缓冲区 HDR 处理（同步，无 yield）
void _processBatchBuffer(
  Float64List buffer,
  int w,
  int h,
  double totalExposure,
  double gamma,
  double rAdj,
  double gAdj,
  double bAdj,
) {
  final numPixels = w * h;

  // Pass 1: sRGB→线性 + 平均亮度
  double sum = 0.0;
  for (int i = 0; i < numPixels * 3; i += 3) {
    final r = _batchSrgbToLinear(buffer[i]);
    final g = _batchSrgbToLinear(buffer[i + 1]);
    final b = _batchSrgbToLinear(buffer[i + 2]);
    buffer[i] = r;
    buffer[i + 1] = g;
    buffer[i + 2] = b;
    sum += 0.2126 * r + 0.7152 * g + 0.0722 * b;
  }

  // Pass 2: 自动伽马
  final mean = sum / numPixels;
  if (mean > 0.001 && mean < 0.999) {
    final autoGamma = math.log(0.5) / math.log(mean);
    final clamped = autoGamma.clamp(0.3, 3.0);
    for (int i = 0; i < numPixels * 3; i++) {
      buffer[i] = math
          .pow(buffer[i].clamp(0.0, double.maxFinite), clamped)
          .toDouble();
    }
  }

  // Pass 3: RGB 调整 + 曝光 + 用户伽马 + sRGB 编码
  for (int i = 0; i < numPixels * 3; i += 3) {
    double r = buffer[i] * rAdj * totalExposure;
    double g = buffer[i + 1] * gAdj * totalExposure;
    double b = buffer[i + 2] * bAdj * totalExposure;

    r = math.pow(r.clamp(0.0, double.maxFinite), gamma).toDouble();
    g = math.pow(g.clamp(0.0, double.maxFinite), gamma).toDouble();
    b = math.pow(b.clamp(0.0, double.maxFinite), gamma).toDouble();

    buffer[i] = _batchLinearToSrgb(r);
    buffer[i + 1] = _batchLinearToSrgb(g);
    buffer[i + 2] = _batchLinearToSrgb(b);
  }
}

/// 向 JPEG 字节流注入 ICC APP2 标记
Uint8List _batchInjectIcc(Uint8List jpegBytes, Uint8List iccData) {
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



/// 在 Isolate 中编码 (真并行)
Uint8List? _encodeInIsolate(List<Object?> args) {
  try {
    final rgba = args[0] as Uint8List;
    final w = args[1] as int, h = args[2] as int;
    final isPng = args[3] as int; // 0=jpeg, 1=png
    final icc = args[4] as Uint8List?;
    final output = img.Image(width: w, height: h, numChannels: 3);
    for (int y = 0; y < h; y++) {
      final rs = y * w * 4;
      for (int x = 0; x < w; x++) {
        final j = rs + x * 4;
        output.setPixelRgba(x, y, rgba[j], rgba[j + 1], rgba[j + 2], 255);
      }
    }
    if (icc != null) {
      output.iccProfile = img.IccProfile('BT.2020', img.IccProfileCompression.none, icc);
    }
    if (isPng == 1) return img.PngEncoder(level: 3).encode(output);
    final savedIcc = output.iccProfile;
    output.iccProfile = null;
    var jpegBytes = img.JpegEncoder(quality: 98).encode(output);
    if (savedIcc != null) jpegBytes = _batchInjectIcc(jpegBytes, savedIcc.decompressed());
    return jpegBytes;
  } catch (_) {
    return null;
  }
}



/// 主屏幕
class HomeScreen extends StatefulWidget {
  const HomeScreen({super.key});

  @override
  State<HomeScreen> createState() => _HomeScreenState();
}

class _HomeScreenState extends State<HomeScreen> {
  // 转换器实例
  late final HdrConverter _converter;

  // 转换设置
  final ConversionSettings _settings = ConversionSettings();

  // 输入图像（单张模式）
  Uint8List? _inputBytes;
  String? _inputFileName;

  // 预览图像
  Uint8List? _previewBytes;
  bool _isPreviewLoading = false;
  double _previewProgress = 0.0;

  // 输出
  bool _isExporting = false;
  final _exportMsgNotifier = ValueNotifier<String?>(null);
  final _fileNameCtrl = TextEditingController(text: 'output');
  String? _saveDir; // 输出目录（Windows 端可自定义）

  // 防抖动计时器
  Timer? _previewDebounce;

  // 批量导出进度节流
  Timer? _progressTimer;

  // ===== 批量模式 =====
  bool _isBatchMode = false;
  List<_BatchFileInfo> _batchFiles = [];
  String? _batchDirPath;
  int _selectedBatchIndex = 0;
  List<String> _fileListOrder = []; // 从 filelist.txt 解析的排序

  @override
  void initState() {
    super.initState();
    _converter = HdrConverter.instance;
    _initConverter();
  }

  /// 预热 Isolate, 避免首次 _executeBatchTask 卡 UI
  static Future<void> _warmupIsolate() async {
    await Isolate.run(() => 42);
  }


  /// 静态方法: Isolate 编码 JPEG + ICC 注入 (不捕获 this)
  static Future<Uint8List?> _runEncodeIsolate(
    Uint8List rgba, int w, int h, int isPng, Uint8List? icc,
  ) {
    return Isolate.run(() => _encodeInIsolate([rgba, w, h, isPng, icc]));
  }



  Future<void> _initConverter() async {
    try {
      await _converter.initialize();
      // 预热 Isolate (后台, 不阻塞)
      _warmupIsolate().ignore();
      if (mounted) {
        setState(() {});
        if (_converter.isGpuAvailable) {
          ScaffoldMessenger.of(context).showSnackBar(
            SnackBar(
              content: Text('GPU 加速已启用: ${_converter.gpuBackendName}'),
              backgroundColor: Colors.green.shade700,
              duration: const Duration(seconds: 2),
            ),
          );
        } else {
          final errMsg = _converter.gpuErrorMessage ?? '未知错误';
          // ignore: avoid_print
          print('[启动] GPU 初始化失败: $errMsg');
          if (mounted) {
            ScaffoldMessenger.of(context).showSnackBar(
              SnackBar(
                content: Text('GPU 初始化失败: $errMsg'),
                backgroundColor: Colors.orange.shade800,
                duration: const Duration(seconds: 8),
              ),
            );
          }
        }
      }
    } catch (e) {
      if (mounted) {
        setState(() {});
        ScaffoldMessenger.of(
          context,
        ).showSnackBar(SnackBar(content: Text('初始化失败: $e')));
      }
    }
  }

  @override
  void dispose() {
    _previewDebounce?.cancel();
    _progressTimer?.cancel();
    _exportMsgNotifier.dispose();
    _fileNameCtrl.dispose();
    super.dispose();
  }

  /// 选择输入图像（单张模式）
  Future<void> _pickInputImage() async {
    _exitBatchMode();
    _converter.dismissHdrPreview();

    const typeGroup = XTypeGroup(
      label: '图片',
      extensions: ['png', 'jpg', 'jpeg', 'webp', 'bmp', 'gif'],
    );
    final file = await openFile(acceptedTypeGroups: [typeGroup]);
    if (file == null) return;

    final bytes = await file.readAsBytes();
    final name = file.name;
    final base = p.basenameWithoutExtension(name);
    _fileNameCtrl.text = '${base}_hdr';

    setState(() {
      _inputBytes = bytes;
      _inputFileName = name;
      _previewBytes = bytes;
      _isPreviewLoading = false;
      _exportMsgNotifier.value = null;
    });

    _generatePreview();
  }

  /// 选择输入目录（批量模式）
  Future<void> _pickInputDirectory() async {
    if (kIsWeb) return; // Web 端暂不支持
    _converter.dismissHdrPreview();

    final dir = await getDirectoryPath();
    if (dir == null) return;

    final dirObj = Directory(dir);
    if (!await dirObj.exists()) return;

    // 扫描图片文件
    final imageExts = {'.png', '.jpg', '.jpeg', '.webp', '.bmp', '.gif'};
    final allFiles = <FileSystemEntity>[];
    await for (final entity in dirObj.list()) {
      allFiles.add(entity);
    }

    final imageFiles = allFiles.whereType<File>().where((f) {
      return imageExts.contains(p.extension(f.path).toLowerCase());
    }).toList();

    if (imageFiles.isEmpty) {
      if (mounted) {
        ScaffoldMessenger.of(
          context,
        ).showSnackBar(const SnackBar(content: Text('目录中没有找到图片文件')));
      }
      return;
    }

    // 读取 filelist.txt（如果存在）
    _fileListOrder = [];
    final fileListPath = '$dir${Platform.pathSeparator}filelist.txt';
    final fileListFile = File(fileListPath);
    if (await fileListFile.exists()) {
      final content = await fileListFile.readAsString();
      // 支持两种格式：纯文件名 或 file 'name.ext'
      _fileListOrder = content
          .split(RegExp(r'[\r\n]+'))
          .map((l) {
            l = l.trim();
            if (l.isEmpty) return '';
            // 提取 file 'name.ext' 或 file "name.ext" 中的文件名
            final fileMatch = RegExp(
              r"""^file\s+['"](.+)['"]\s*$""",
            ).firstMatch(l);
            if (fileMatch != null) return fileMatch.group(1)!;
            return l;
          })
          .where((l) => l.isNotEmpty)
          .toList();
    }

    // 按 filelist.txt 排序
    if (_fileListOrder.isNotEmpty) {
      imageFiles.sort((a, b) {
        final aName = p.basename(a.path);
        final bName = p.basename(b.path);
        final aIdx = _fileListOrder.indexOf(aName);
        final bIdx = _fileListOrder.indexOf(bName);
        if (aIdx != -1 && bIdx != -1) return aIdx.compareTo(bIdx);
        if (aIdx != -1) return -1;
        if (bIdx != -1) return 1;
        return aName.compareTo(bName);
      });
    } else {
      imageFiles.sort(
        (a, b) => p.basename(a.path).compareTo(p.basename(b.path)),
      );
    }

    // 读取所有图片
    final batchFiles = <_BatchFileInfo>[];
    for (final f in imageFiles) {
      try {
        final bytes = await f.readAsBytes();
        final name = p.basename(f.path);
        final baseName = p.basenameWithoutExtension(f.path);
        batchFiles.add(
          _BatchFileInfo(name: name, baseName: baseName, bytes: bytes),
        );
      } catch (e) {
        // 跳过无法读取的文件
      }
    }

    if (batchFiles.isEmpty) return;

    setState(() {
      _isBatchMode = true;
      _batchFiles = batchFiles;
      _batchDirPath = dir;
      _selectedBatchIndex = 0;
      _exportMsgNotifier.value = null;
      // 默认预览第一张
      _inputBytes = batchFiles[0].bytes;
      _inputFileName = batchFiles[0].name;
      _previewBytes = batchFiles[0].bytes;
      _isPreviewLoading = false;
    });

    _generateBatchPreview(0);
  }

  /// 退出批量模式
  void _exitBatchMode() {
    if (_isBatchMode) {
      setState(() {
        _isBatchMode = false;
        _batchFiles = [];
        _batchDirPath = null;
        _fileListOrder = [];
      });
    }
  }

  /// 选择批量文件预览
  void _selectBatchFile(int index) {
    if (index < 0 || index >= _batchFiles.length) return;
    setState(() {
      _selectedBatchIndex = index;
      _inputBytes = _batchFiles[index].bytes;
      _inputFileName = _batchFiles[index].name;
      _previewBytes =
          _batchFiles[index].previewBytes ?? _batchFiles[index].bytes;
    });
    _generateBatchPreview(index);
  }

  /// 生成批量文件中某一项的预览
  void _generateBatchPreview(int index) {
    _previewDebounce?.cancel();
    _previewDebounce = Timer(const Duration(milliseconds: 300), () async {
      if (index < 0 || index >= _batchFiles.length) return;
      final fileInfo = _batchFiles[index];

      setState(() => _isPreviewLoading = true);
      _previewProgress = 0.0;

      try {
        final preview = await _converter.convertForPreview(
          inputBytes: fileInfo.bytes,
          settings: _settings,
          onProgress: (p) {
            _previewProgress = p;
            if (mounted) setState(() {});
          },
        );
        if (mounted) {
          fileInfo.previewBytes = preview;
          setState(() {
            _previewBytes = preview;
            _isPreviewLoading = false;
            _previewProgress = 0.0;
          });
          if (kIsWeb) _converter.openHdrPreview(preview);
        }
      } catch (e) {
        if (mounted) {
          setState(() => _isPreviewLoading = false);
          ScaffoldMessenger.of(
            context,
          ).showSnackBar(SnackBar(content: Text('预览生成失败: $e')));
        }
      }
    });
  }

  /// 生成预览（带防抖动）
  void _generatePreview() {
    _previewDebounce?.cancel();
    _previewDebounce = Timer(const Duration(milliseconds: 300), () async {
      if (_inputBytes == null) return;

      setState(() => _isPreviewLoading = true);

      _previewProgress = 0.0;
      try {
        final preview = await _converter.convertForPreview(
          inputBytes: _inputBytes!,
          settings: _settings,
          onProgress: (p) {
            _previewProgress = p;
            if (mounted) setState(() {});
          },
        );
        if (mounted) {
          setState(() {
            _previewBytes = preview;
            _isPreviewLoading = false;
            _previewProgress = 0.0;
          });
          // Web 端自动弹出 HDR 覆盖层
          if (kIsWeb) {
            _converter.openHdrPreview(preview);
          }
        }
      } catch (e) {
        if (mounted) {
          setState(() => _isPreviewLoading = false);
          ScaffoldMessenger.of(
            context,
          ).showSnackBar(SnackBar(content: Text('预览生成失败: $e')));
        }
      }
    });
  }

  /// 设置变更回调（拖动滑块时快速同步值，不触发预览）
  void _onSettingsChanged(ConversionSettings settings) {
    setState(() {
      final savedFormat = _settings.outputFormat;
      _syncSettings(settings);
      _settings.outputFormat = savedFormat; // 输出格式由专用回调管理
    });
  }

  /// 输出格式变更回调（来自输出标签页）
  void _onFormatChanged(OutputFormat format) {
    setState(() {
      _settings.outputFormat = format;
    });
  }

  /// 设置变更结束回调（松开滑块时生成预览）
  void _onSettingsChangeEnd(ConversionSettings settings) {
    setState(() {
      final savedFormat = _settings.outputFormat;
      _syncSettings(settings);
      _settings.outputFormat = savedFormat;
    });
    if (_isBatchMode && _batchFiles.isNotEmpty) {
      _generateBatchPreview(_selectedBatchIndex);
    } else {
      _generatePreview();
    }
  }

  /// 同步设置值
  void _syncSettings(ConversionSettings settings) {
    _settings.hdrIntensity = settings.hdrIntensity;
    _settings.fineTuneBrightness = settings.fineTuneBrightness;
    _settings.gamma = settings.gamma;
    _settings.rgbAdjustment.red = settings.rgbAdjustment.red;
    _settings.rgbAdjustment.green = settings.rgbAdjustment.green;
    _settings.rgbAdjustment.blue = settings.rgbAdjustment.blue;
    _settings.outputFormat = settings.outputFormat;
  }

  /// 选择输出目录（Windows 端）
  Future<void> _pickSaveDir() async {
    if (kIsWeb) return;
    final dir = await getDirectoryPath();
    if (dir != null) {
      setState(() => _saveDir = dir);
    }
  }

  /// 导出 HDR 图像
  Future<void> _exportHdr() async {
    if (_inputBytes == null) return;

    setState(() {
      _isExporting = true;
      _exportMsgNotifier.value = null;
    });

    // 让出事件循环，确保 UI 刷新后再开始重活儿
    await Future<void>.delayed(const Duration(milliseconds: 50));

    try {
      Uint8List? outputBytes;

      // === 尝试 GPU 路径 (主线程, 不阻塞) ===
      await _converter.initialize();
      if (_converter.isGpuAvailable) {
        _exportMsgNotifier.value =
            '处理中 (GPU: ${_converter.gpuBackendName})...';
        await Future<void>.delayed(const Duration(milliseconds: 1));

        try {
          outputBytes = await _converter.convertSdrToHdr(
            inputBytes: _inputBytes!,
            settings: _settings,
            onProgress: (p) {
              if (mounted) {
                _exportMsgNotifier.value =
                    '处理中 (GPU: ${_converter.gpuBackendName}) ${(p * 100).toInt()}%';
              }
            },
          );
          // ignore: avoid_print
          print('[导出] 实际后端: ${_converter.lastUsedBackend}');
          if (_converter.lastUsedBackend.contains('GPU失败')) {
            final gpuErr = _converter.gpuErrorMessage;
            if (mounted && gpuErr != null) {
              _exportMsgNotifier.value = 'GPU 错误: $gpuErr，回退 CPU';
            }
          }
        } catch (e) {
          // GPU 失败, 显示错误后回退 CPU
          final gpuErr = _converter.gpuErrorMessage ?? '未知错误';
          // ignore: avoid_print
          print('[导出] GPU 失败: $gpuErr');
          if (mounted) {
            _exportMsgNotifier.value = 'GPU 错误: $gpuErr，回退 CPU';
            ScaffoldMessenger.of(context).showSnackBar(
              SnackBar(
                content: Text('GPU 处理失败: $gpuErr', style: const TextStyle(fontSize: 12)),
                backgroundColor: Colors.red.shade800,
                duration: const Duration(seconds: 6),
              ),
            );
          }
          outputBytes = null;
        }
      }

      // === CPU 回退路径 (Isolate) ===
      if (outputBytes == null) {
        // ignore: avoid_print
        print('[导出] 实际后端: CPU (Isolate)');
        // 预加载 ICC 配置
        Uint8List? iccProfile;
        try {
          iccProfile = (await rootBundle.load(
            'assets/2020_profile.icc',
          )).buffer.asUint8List();
        } catch (_) {}

        if (!mounted) return;

        _exportMsgNotifier.value = '处理中 (CPU)...';

        final totalExposure = _settings.totalExposure - 1;
        final task = _BatchTask(
          index: 0,
          inputBytes: _inputBytes!,
          totalExposure: totalExposure,
          gamma: _settings.gamma,
          rAdj: _settings.rgbAdjustment.red,
          gAdj: _settings.rgbAdjustment.green,
          bAdj: _settings.rgbAdjustment.blue,
          outputFormat: _settings.outputFormat,
          iccProfile: iccProfile,
        );

        // 在独立 isolate 中执行 HDR 转换
        final result = await _executeBatchTask(task);

        if (!mounted) return;

        if (result.outputBytes == null) {
          throw Exception(result.error ?? '转换失败');
        }

        outputBytes = result.outputBytes;
      }

      if (!mounted) return;
      final ext = _converter.getOutputExtension(_settings);
      final baseName = _fileNameCtrl.text.trim();
      final safeName = baseName.isEmpty ? 'output' : baseName;
      final defaultName = '$safeName.$ext';

      // Windows 端有保存目录 → 直接自动保存
      if (!kIsWeb && _saveDir != null) {
        final path = '$_saveDir\\$defaultName';
        _exportMsgNotifier.value = '正在保存...';
        await saveFile(path, outputBytes!);
        if (mounted) {
          _exportMsgNotifier.value =
              '已保存: $defaultName (${_converter.lastUsedBackend})';
          setState(() => _isExporting = false);
        }
        return;
      }

      // Web 端或首次导出：弹出保存对话框
      final destination = await getSaveLocation(
        suggestedName: defaultName,
        acceptedTypeGroups: [
          XTypeGroup(label: 'HDR 图像', extensions: [ext]),
        ],
      );

      if (destination != null) {
        _exportMsgNotifier.value = '正在保存...';

        if (kIsWeb) {
          // Web：直接下载
          await saveFile(defaultName, outputBytes!);
        } else {
          // Windows：记录目录，用统一路径保存
          _saveDir = p.dirname(destination.path);
          final savePath = '$_saveDir\\$defaultName';
          await saveFile(savePath, outputBytes!);
        }

        if (mounted) {
          _exportMsgNotifier.value =
              '已保存: $defaultName (${_converter.lastUsedBackend})';
          setState(() => _isExporting = false);
        }
      } else {
        if (mounted) setState(() => _isExporting = false);
      }
    } catch (e) {
      _exportMsgNotifier.value = '导出失败: $e';
      if (mounted) {
        setState(() => _isExporting = false);
      }
    }
  }

  /// 批量导出为 ZIP
  Future<void> _exportBatchZip() async {
    if (!_isBatchMode || _batchFiles.isEmpty) return;
    if (kIsWeb) return; // Web 端暂不支持

    setState(() {
      _isExporting = true;
    });
    _exportMsgNotifier.value = '准备批量处理...';
    // 让出事件循环，确保 UI 刷新后再开始重活儿
    await Future<void>.delayed(const Duration(milliseconds: 50));

    try {
      final ext = _converter.getOutputExtension(_settings);
      final total = _batchFiles.length;

      // 预加载 ICC 配置（从主 isolate 读取后传给 worker isolate）
      Uint8List? iccProfile;
      try {
        await _converter.initialize();
        iccProfile = (await rootBundle.load(
          'assets/2020_profile.icc',
        )).buffer.asUint8List();
      } catch (_) {}

      // Phase 1: GPU 处理 (串行, GPU Isolate 单线程处理)
      final pending = <int, ProcessedImage?>{};
      final totalExp = _settings.totalExposure - 1;
      for (int i = 0; i < total && _converter.isGpuAvailable; i++) {
        if (!mounted) return;
        _exportMsgNotifier.value = 'GPU ${i + 1}/$total: ${_batchFiles[i].name}...';
        await Future<void>.delayed(const Duration(milliseconds: 1));
        try {
          final p = await _converter.processImageBytes(_batchFiles[i].bytes, _settings);
          if (p != null) pending[i] = p;
        } catch (_) {}
      }

      // Phase 2: 并发 Isolate 编码 (吃满 CPU 核)
      final results = <_BatchResult>[];
      final isPng = _settings.outputFormat == OutputFormat.hdrPng;

      final cpuCount = Platform.numberOfProcessors;
      final maxConcurrent = (cpuCount / 3).round().clamp(2, 8);
      int encNext = 0;

      Future<void> encodeWorker() async {
        while (encNext < total) {
          final i = encNext++;
          if (!mounted) return;
          final processed = pending[i];
          Uint8List? bytes;

          if (processed != null) {
            // GPU + Isolate 编码 (PNG/JPEG 统一走 Isolate, 真 CPU 并行)
            _exportMsgNotifier.value = '编码 ${i + 1}/$total: ${_batchFiles[i].name}...';
            final pngFlag = isPng ? 1 : 0;
            bytes = await _runEncodeIsolate(processed.rgba, processed.width, processed.height, pngFlag, iccProfile);
          } else {
            // CPU 回退: 完整管线在 Isolate 中执行
            _exportMsgNotifier.value = 'CPU ${i + 1}/$total: ${_batchFiles[i].name}...';
            final task = _BatchTask(
              index: i, inputBytes: _batchFiles[i].bytes,
              totalExposure: totalExp, gamma: _settings.gamma,
              rAdj: _settings.rgbAdjustment.red, gAdj: _settings.rgbAdjustment.green,
              bAdj: _settings.rgbAdjustment.blue,
              outputFormat: _settings.outputFormat, iccProfile: iccProfile,
            );
            final r = await _executeBatchTask(task);
            bytes = r.outputBytes;
          }

          if (bytes != null) {
            _batchFiles[i].outputBytes = bytes;
            _batchFiles[i].isProcessed = true;
            results.add(_BatchResult(index: i, outputBytes: bytes));
          } else {
            _batchFiles[i].isProcessed = false;
          }
          // 释放 RGBA 原始数据 (单张可达 8~33MB, 300 张不释放会 OOM)
          pending[i] = null;
          // yield 让主线程处理 UI 事件
          await Future<void>.delayed(const Duration(milliseconds: 2));
        }
      }

      final workers = <Future<void>>[];
      for (int i = 0; i < maxConcurrent && i < total; i++) {
        workers.add(encodeWorker());
        await Future<void>.delayed(const Duration(milliseconds: 5));
      }
      await Future.wait(workers);

      if (!mounted) return;

      // 按索引排序后, 直接流式写入 ZIP 文件 (store 模式, 图片已压缩无需再压)
      results.sort((a, b) => a.index.compareTo(b.index));
      final dirName = p.basename(_batchDirPath ?? 'batch_output');
      final zipName = '${dirName}_hdr.zip';

      String zipPath;
      bool saveDirect;
      if (_saveDir != null) {
        zipPath = '$_saveDir\\$zipName';
        saveDirect = true;
      } else {
        zipPath = '${Directory.systemTemp.path}\\${DateTime.now().millisecondsSinceEpoch}_$zipName';
        saveDirect = false;
      }

      _progressTimer?.cancel();
      _progressTimer = null;
      _exportMsgNotifier.value = '正在打包 ZIP...';

      // 流式写入 ZIP (不压缩, 图片已经是 PNG/JPEG, 不创建内存 Archive)
      final zipEncoder = ZipFileEncoder();
      zipEncoder.create(zipPath, level: ZipFileEncoder.store);
      for (final r in results) {
        if (r.outputBytes != null) {
          final outName = '${_batchFiles[r.index].baseName}_hdr.$ext';
          zipEncoder.addArchiveFile(ArchiveFile(outName, r.outputBytes!.length, r.outputBytes!));
        }
      }

      // 写入 filelist.txt
      final orderedNames = <String>[];
      for (final origName in _fileListOrder) {
        final baseName = origName.replaceAll(RegExp(r'\.[^.]+$'), '');
        final outName = '${baseName}_hdr.$ext';
        orderedNames.add(outName);
      }
      for (final r in results) {
        if (r.outputBytes != null) {
          final outName = '${_batchFiles[r.index].baseName}_hdr.$ext';
          if (!orderedNames.contains(outName)) orderedNames.add(outName);
        }
      }
      if (orderedNames.isNotEmpty) {
        final sb = StringBuffer();
        for (final n in orderedNames) {
          sb.writeln("file '$n'");
        }
        final filelistBytes = Uint8List.fromList(sb.toString().codeUnits);
        zipEncoder.addArchiveFile(ArchiveFile('filelist.txt', filelistBytes.length, filelistBytes));
      }
      zipEncoder.closeSync();

      // 保存/移动 ZIP
      _exportMsgNotifier.value = '正在保存 ZIP...';
      if (saveDirect) {
        if (mounted) {
          _exportMsgNotifier.value = '已保存: $zipName';
          setState(() => _isExporting = false);
        }
      } else {
        final destination = await getSaveLocation(
          suggestedName: zipName,
          acceptedTypeGroups: [
            XTypeGroup(label: 'ZIP 压缩包', extensions: ['zip']),
          ],
        );
        if (destination != null) {
          _saveDir = p.dirname(destination.path);
          await File(zipPath).copy(destination.path);
          if (mounted) {
            _exportMsgNotifier.value = '已保存: $zipName';
            setState(() => _isExporting = false);
          }
        } else {
          if (mounted) setState(() => _isExporting = false);
        }
        try { await File(zipPath).delete(); } catch (_) {}
      }
    } catch (e) {
      _progressTimer?.cancel();
      _progressTimer = null;
      _exportMsgNotifier.value = '批量导出失败: $e';
      if (mounted) {
        setState(() {
          _isExporting = false;
        });
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    final isWide = MediaQuery.of(context).size.width > 700;

    return Scaffold(
      appBar: AppBar(
        title: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Text(_isBatchMode ? '批量处理' : 'SDR → HDR 转换'),
            const SizedBox(width: 8),
            // GPU 状态指示
            Container(
              padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 2),
              decoration: BoxDecoration(
                color: _converter.isGpuAvailable
                    ? Colors.green.shade700
                    : Colors.grey.shade600,
                borderRadius: BorderRadius.circular(4),
              ),
              child: Text(
                _converter.isGpuAvailable
                    ? 'GPU: ${_converter.gpuBackendName}'
                    : 'CPU',
                style: const TextStyle(
                  fontSize: 10,
                  color: Colors.white,
                  fontWeight: FontWeight.bold,
                ),
              ),
            ),
          ],
        ),
        centerTitle: true,
        elevation: 1,
        actions: _isBatchMode
            ? [
                IconButton(
                  icon: const Icon(Icons.close),
                  tooltip: '退出批量模式',
                  onPressed: () {
                    _exitBatchMode();
                    setState(() {
                      _inputBytes = null;
                      _inputFileName = null;
                      _previewBytes = null;
                      _exportMsgNotifier.value = null;
                    });
                  },
                ),
              ]
            : null,
      ),
      body: isWide ? _buildWideLayout() : _buildNarrowLayout(),
    );
  }

  /// 宽屏排版（电脑）
  Widget _buildWideLayout() {
    if (_isBatchMode) {
      return Column(
        children: [
          _buildInputSection(),
          Expanded(
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                // 文件列表侧栏
                SizedBox(width: 220, child: _buildBatchFileList()),
                const VerticalDivider(width: 1),
                // 设置面板
                SizedBox(
                  width: 320,
                  child: SettingsPanel(
                    settings: _settings.copy(),
                    onChanged: _onSettingsChanged,
                    onChangeEnd: _onSettingsChangeEnd,
                    onFormatChanged: _onFormatChanged,
                    fileNameController: _fileNameCtrl,
                    saveDir: _saveDir,
                    onPickSaveDir: kIsWeb ? null : _pickSaveDir,
                  ),
                ),
                const VerticalDivider(width: 1),
                // 预览 + 导出
                Expanded(
                  child: Column(
                    children: [
                      Expanded(child: _buildPreviewSection()),
                      _buildOutputSection(),
                    ],
                  ),
                ),
              ],
            ),
          ),
        ],
      );
    }

    return Column(
      children: [
        _buildInputSection(),
        Expanded(
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              // 左侧：设置面板 + 输出文件名
              SizedBox(
                width: 360,
                child: Column(
                  children: [
                    Expanded(
                      child: SettingsPanel(
                        settings: _settings.copy(),
                        onChanged: _onSettingsChanged,
                        onChangeEnd: _onSettingsChangeEnd,
                        onFormatChanged: _onFormatChanged,
                        fileNameController: _fileNameCtrl,
                        saveDir: _saveDir,
                        onPickSaveDir: kIsWeb ? null : _pickSaveDir,
                      ),
                    ),
                  ],
                ),
              ),
              const VerticalDivider(width: 1),
              // 右侧：预览 + 导出
              Expanded(
                child: Column(
                  children: [
                    Expanded(child: _buildPreviewSection()),
                    _buildOutputSection(),
                  ],
                ),
              ),
            ],
          ),
        ),
      ],
    );
  }

  /// 窄屏排版（手机）
  Widget _buildNarrowLayout() {
    if (_isBatchMode) {
      return Column(
        children: [
          _buildInputSection(),
          SizedBox(height: 120, child: _buildBatchFileList()),
          const Divider(height: 1),
          Expanded(child: _buildPreviewSection()),
          SettingsPanel(
            settings: _settings.copy(),
            onChanged: _onSettingsChanged,
            onChangeEnd: _onSettingsChangeEnd,
            onFormatChanged: _onFormatChanged,
            fileNameController: _fileNameCtrl,
            saveDir: _saveDir,
            onPickSaveDir: kIsWeb ? null : _pickSaveDir,
          ),
          _buildOutputSection(),
        ],
      );
    }

    return Column(
      children: [
        _buildInputSection(),
        Expanded(child: _buildPreviewSection()),
        SettingsPanel(
          settings: _settings.copy(),
          onChanged: _onSettingsChanged,
          onChangeEnd: _onSettingsChangeEnd,
          onFormatChanged: _onFormatChanged,
          fileNameController: _fileNameCtrl,
          saveDir: _saveDir,
          onPickSaveDir: kIsWeb ? null : _pickSaveDir,
        ),
        _buildOutputSection(),
      ],
    );
  }

  /// 批量文件列表
  Widget _buildBatchFileList() {
    final theme = Theme.of(context);
    return Material(
      color: theme.colorScheme.surfaceContainerLow,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Padding(
            padding: const EdgeInsets.fromLTRB(12, 10, 12, 4),
            child: Text(
              '文件列表 (${_batchFiles.length})',
              style: theme.textTheme.labelMedium?.copyWith(
                color: theme.colorScheme.primary,
                fontWeight: FontWeight.bold,
              ),
            ),
          ),
          const Divider(height: 1),
          Expanded(
            child: ListView.builder(
              itemCount: _batchFiles.length,
              itemBuilder: (context, index) {
                final fileInfo = _batchFiles[index];
                final isSelected = index == _selectedBatchIndex;
                return ListTile(
                  dense: true,
                  selected: isSelected,
                  selectedColor: theme.colorScheme.onPrimaryContainer,
                  selectedTileColor: theme.colorScheme.primaryContainer,
                  leading: fileInfo.previewBytes != null
                      ? ClipRRect(
                          borderRadius: BorderRadius.circular(4),
                          child: Image.memory(
                            fileInfo.previewBytes!,
                            width: 32,
                            height: 32,
                            fit: BoxFit.cover,
                          ),
                        )
                      : Icon(
                          Icons.image,
                          size: 24,
                          color: theme.colorScheme.onSurfaceVariant,
                        ),
                  title: Text(
                    fileInfo.name,
                    overflow: TextOverflow.ellipsis,
                    style: const TextStyle(fontSize: 12),
                    maxLines: 1,
                  ),
                  onTap: () => _selectBatchFile(index),
                );
              },
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildInputSection() {
    final theme = Theme.of(context);
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 8),
      decoration: BoxDecoration(
        color: theme.colorScheme.surfaceContainerLow,
        border: const Border(
          bottom: BorderSide(color: Colors.grey, width: 0.5),
        ),
      ),
      child: Row(
        children: [
          Icon(
            _isBatchMode ? Icons.folder : Icons.image,
            color: theme.colorScheme.primary,
          ),
          const SizedBox(width: 8),
          Expanded(
            child: Text(
              _isBatchMode
                  ? (_batchDirPath ?? '未选择目录')
                  : (_inputFileName ?? '未选择图片'),
              overflow: TextOverflow.ellipsis,
              style: TextStyle(
                fontSize: _isBatchMode ? 13 : 14,
                color: _inputFileName != null || _batchDirPath != null
                    ? null
                    : Colors.grey,
              ),
            ),
          ),
          const SizedBox(width: 8),
          // 批量模式只显示目录路径，不显示操作按钮
          if (!_isBatchMode) ...[
            if (!kIsWeb)
              Padding(
                padding: const EdgeInsets.only(right: 8),
                child: OutlinedButton.icon(
                  onPressed: _pickInputDirectory,
                  icon: const Icon(Icons.folder, size: 18),
                  label: const Text('目录'),
                ),
              ),
            FilledButton.tonalIcon(
              onPressed: _pickInputImage,
              icon: const Icon(Icons.folder_open, size: 18),
              label: const Text('打开'),
            ),
          ],
        ],
      ),
    );
  }

  Widget _buildPreviewSection() {
    if (_isPreviewLoading) {
      return Center(
        child: Column(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            CircularProgressIndicator(
              value: _previewProgress > 0 ? _previewProgress : null,
            ),
            const SizedBox(height: 16),
            Text(
              _previewProgress > 0
                  ? '生成预览中 ${(_previewProgress * 100).toInt()}%'
                  : '生成预览中...',
            ),
          ],
        ),
      );
    }

    if (_previewBytes != null) {
      return Center(
        child: InteractiveViewer(
          child: Padding(
            padding: const EdgeInsets.all(8),
            child: ClipRRect(
              borderRadius: BorderRadius.circular(8),
              child: Image.memory(
                _previewBytes!,
                fit: BoxFit.contain,
                errorBuilder: (_, _, _) => const Center(child: Text('预览加载失败')),
              ),
            ),
          ),
        ),
      );
    }

    if (_inputBytes != null) {
      return Center(
        child: Column(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            Icon(Icons.hourglass_empty, size: 48, color: Colors.grey.shade400),
            const SizedBox(height: 8),
            Text('调整设置以生成预览', style: TextStyle(color: Colors.grey.shade600)),
          ],
        ),
      );
    }

    return Center(
      child: Column(
        mainAxisAlignment: MainAxisAlignment.center,
        children: [
          Icon(
            Icons.cloud_upload_outlined,
            size: 64,
            color: Colors.grey.shade400,
          ),
          const SizedBox(height: 16),
          Text('点击上方"打开"按钮选择图片', style: TextStyle(color: Colors.grey.shade600)),
          const SizedBox(height: 8),
          Text(
            '支持 PNG / JPG / WebP / BMP / GIF',
            style: TextStyle(color: Colors.grey.shade500, fontSize: 12),
          ),
        ],
      ),
    );
  }

  Widget _buildOutputSection() {
    final theme = Theme.of(context);
    return Container(
      padding: const EdgeInsets.all(16),
      decoration: BoxDecoration(
        color: theme.colorScheme.surfaceContainerLow,
        border: const Border(top: BorderSide(color: Colors.grey, width: 0.5)),
      ),
      child: SafeArea(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            // 导出进度条
            if (_isExporting)
              Padding(
                padding: const EdgeInsets.only(bottom: 8),
                child: Column(
                  children: [
                    const LinearProgressIndicator(),
                    const SizedBox(height: 6),
                    ValueListenableBuilder<String?>(
                      valueListenable: _exportMsgNotifier,
                      builder: (context, msg, _) {
                        return Text(
                          msg ?? '处理中...',
                          style: TextStyle(
                            color: Colors.grey.shade600,
                            fontSize: 12,
                          ),
                        );
                      },
                    ),
                  ],
                ),
              ),
            // 完成/失败消息
            ValueListenableBuilder<String?>(
              valueListenable: _exportMsgNotifier,
              builder: (context, msg, _) {
                if (msg == null || _isExporting) return const SizedBox.shrink();
                return Padding(
                  padding: const EdgeInsets.only(bottom: 8),
                  child: Text(
                    msg,
                    style: TextStyle(
                      color: msg.contains('失败') ? Colors.red : Colors.green,
                      fontSize: 13,
                    ),
                    textAlign: TextAlign.center,
                  ),
                );
              },
            ),
            Row(
              children: [
                // HDR 预览按钮（仅 Web）
                if (kIsWeb && !_isBatchMode)
                  Expanded(
                    child: Padding(
                      padding: const EdgeInsets.only(right: 8),
                      child: OutlinedButton.icon(
                        onPressed: (_previewBytes != null)
                            ? () => _openHdrPreview()
                            : null,
                        icon: const Icon(Icons.visibility, size: 18),
                        label: const Text('HDR 预览'),
                      ),
                    ),
                  ),
                // 导出按钮
                Expanded(
                  child: SizedBox(
                    height: 48,
                    child: FilledButton.icon(
                      onPressed: _isBatchMode
                          ? (_batchFiles.isNotEmpty && !_isExporting
                                ? _exportBatchZip
                                : null)
                          : (_inputBytes != null && !_isExporting
                                ? _exportHdr
                                : null),
                      icon: _isExporting
                          ? const SizedBox(
                              width: 18,
                              height: 18,
                              child: CircularProgressIndicator(
                                strokeWidth: 2,
                                color: Colors.white,
                              ),
                            )
                          : Icon(_isBatchMode ? Icons.archive : Icons.save_alt),
                      label: Text(
                        _isExporting
                            ? '导出中...'
                            : (_isBatchMode ? '批量导出 ZIP' : '导出 HDR 图像'),
                      ),
                    ),
                  ),
                ),
              ],
            ),
          ],
        ),
      ),
    );
  }

  /// 打开 HDR 预览（Web 端新标签页）
  void _openHdrPreview() {
    if (_previewBytes == null) return;
    _converter.openHdrPreview(_previewBytes!);
  }
}
