import 'dart:ffi';
import 'dart:io' show Platform;
import 'dart:typed_data';
import 'package:ffi/ffi.dart';

// =====================================================================
// hdr_gpu.dll FFI 绑定
// 提供 GPU 加速的 HDR 转换 (DirectCompute + CUDA 自动检测)
// =====================================================================

/// GPU 后端类型
class HdrGpuBackend {
  static const none = 0;
  static const cuda = 1;
  static const directCompute = 2;

  static String name(int backend) {
    switch (backend) {
      case cuda:
        return 'CUDA';
      case directCompute:
        return 'DirectCompute';
      default:
        return 'None';
    }
  }
}

/// GPU 加速引擎 — 封装 hdr_gpu.dll 的 C API
class HdrGpuEngine {
  static HdrGpuEngine? _instance;

  DynamicLibrary? _lib;
  bool _initialized = false;
  bool _previewLoaded = false;
  int _activeBackend = HdrGpuBackend.none;
  String? _lastError;

  // 函数指针 (asFunction 返回的是 Dart 函数, 不是 Function() 包裹的)
  late int Function(int backend) _nativeInit;
  late int Function(
    Pointer<Uint8> input,
    int width,
    int height,
    Pointer<Uint8> output,
    double totalExposure,
    double gamma,
    double rAdj,
    double gAdj,
    double bAdj,
  )
  _nativeProcess;
  late Pointer<Utf8> Function() _nativeError;
  late void Function() _nativeCleanup;
  late int Function() _nativeBackend;

  // HDR 预览函数指针
  late int Function() _nativePreviewCheckSystemHdr;
  late int Function(int parentHwnd, int width, int height) _nativePreviewCreate;
  late int Function(int x, int y, int width, int height) _nativePreviewSetPosition;
  late int Function(
    Pointer<Uint8> rgba,
    int width,
    int height,
  )
  _nativePreviewShow;
  late void Function() _nativePreviewHide;
  late void Function() _nativePreviewDestroy;
  late int Function() _nativePreviewIsHdrAvailable;
  late int Function() _nativePreviewIsVisible;
  late Pointer<Utf8> Function() _nativePreviewError;

  HdrGpuEngine._();

  static HdrGpuEngine get instance {
    _instance ??= HdrGpuEngine._();
    return _instance!;
  }

  /// 是否已初始化且可用
  bool get isAvailable => _initialized;

  /// 当前活动的 GPU 后端
  int get activeBackend => _activeBackend;

  /// 后端名称
  String get backendName => HdrGpuBackend.name(_activeBackend);

  /// 上次错误消息
  String? get lastError => _lastError;

  /// 加载并初始化 GPU 引擎
  Future<bool> initialize() async {
    if (_initialized) return true;

    // 查找 DLL
    try {
      // Windows: DLL 在可执行文件同目录
      String libPath;
      if (Platform.isWindows) {
        // 先尝试从程序目录加载（部署后）
        libPath = 'hdr_gpu.dll';
      } else {
        _lastError = 'GPU acceleration is only supported on Windows';
        return false;
      }

      _lib = DynamicLibrary.open(libPath);
      // ignore: avoid_print
      print('[HDR GPU] DLL loaded successfully');
    } catch (e) {
      _lastError = 'Failed to load hdr_gpu.dll: $e';
      // ignore: avoid_print
      print('[HDR GPU] $_lastError');
      return false;
    }

    try {
      // 绑定函数 (显式指定 asFunction 的类型参数)
      _nativeInit = _lib!
          .lookup<NativeFunction<Int32 Function(Int32)>>('hdr_gpu_init')
          .asFunction<int Function(int)>();

      _nativeProcess = _lib!
          .lookup<
            NativeFunction<
              Int32 Function(
                Pointer<Uint8>,
                Int32,
                Int32,
                Pointer<Uint8>,
                Float,
                Float,
                Float,
                Float,
                Float,
              )
            >
          >('hdr_gpu_process')
          .asFunction<
            int Function(
              Pointer<Uint8>,
              int,
              int,
              Pointer<Uint8>,
              double,
              double,
              double,
              double,
              double,
            )
          >();
      _nativeCleanup = _lib!
          .lookup<NativeFunction<Void Function()>>('hdr_gpu_cleanup')
          .asFunction<void Function()>();

      _nativeBackend = _lib!
          .lookup<NativeFunction<Int32 Function()>>('hdr_gpu_backend')
          .asFunction<int Function()>();

      _nativeError = _lib!
          .lookup<NativeFunction<Pointer<Utf8> Function()>>('hdr_gpu_error')
          .asFunction<Pointer<Utf8> Function()>();

      // 绑定 HDR 预览函数
      _bindPreviewFunctions();
    } catch (e) {
      _lastError = 'Failed to bind functions: $e';
      _lib = null;
      return false;
    }

    // 自动初始化 (优先 CUDA, 回退 DirectCompute)
    final result = _nativeInit(HdrGpuBackend.none);
    if (result != 0) {
      _lastError = _readError();
      // ignore: avoid_print
      print('[HDR GPU] Init failed: $_lastError');
      _lib = null;
      return false;
    }

    _activeBackend = _nativeBackend();
    // ignore: avoid_print
    print(
      '[HDR GPU] Backend initialized: ${HdrGpuBackend.name(_activeBackend)}',
    );
    _initialized = true;
    return true;
  }

  /// 处理图像
  Uint8List? process(
    Uint8List input, {
    required int width,
    required int height,
    required double totalExposure,
    required double gamma,
    required double rAdj,
    required double gAdj,
    required double bAdj,
  }) {
    if (!_initialized) {
      _lastError = 'GPU engine not initialized';
      return null;
    }

    final output = Uint8List(width * height * 4);

    final inputPtr = calloc<Uint8>(input.length);
    final outputPtr = calloc<Uint8>(output.length);

    try {
      inputPtr.asTypedList(input.length).setAll(0, input);

      final result = _nativeProcess(
        inputPtr,
        width,
        height,
        outputPtr,
        totalExposure,
        gamma,
        rAdj,
        gAdj,
        bAdj,
      );

      if (result != 0) {
        _lastError = _readError();
        // ignore: avoid_print
        print('[HDR GPU] Process failed: $_lastError');
        return null;
      }

      for (int i = 0; i < output.length; i++) {
        output[i] = outputPtr.asTypedList(output.length)[i];
      }
      return output;
    } finally {
      calloc.free(inputPtr);
      calloc.free(outputPtr);
    }
  }

  /// 仅加载 DLL 用于 HDR 预览 (不需要计算后端初始化)
  ///
  /// 返回 true 表示 DLL 已加载且预览函数可用。
  bool loadPreviewOnly() {
    if (_previewLoaded) return true;
    if (_lib != null) {
      // DLL 已加载（可能来自 compute init），直接标记可用
      _previewLoaded = true;
      return true;
    }
    try {
      _lib = DynamicLibrary.open('hdr_gpu.dll');
      _bindPreviewFunctions();
      _previewLoaded = true;
      return true;
    } catch (e) {
      _lastError = 'Failed to load hdr_gpu.dll for preview: $e';
      return false;
    }
  }

  /// 释放所有 GPU 资源
  void cleanup() {
    if (_initialized) {
      _nativeCleanup();
    }
    _initialized = false;
    _activeBackend = HdrGpuBackend.none;
    _lib = null;
  }

  /// 读取错误消息
  String _readError() {
    final ptr = _nativeError();
    if (ptr == nullptr) return 'Unknown error';
    return ptr.toDartString();
  }

  /// 绑定 HDR 预览函数 (每个 try 独立, 允许部分失败)
  void _bindPreviewFunctions() {
    try {
      _nativePreviewCheckSystemHdr = _lib!
          .lookup<NativeFunction<Int32 Function()>>(
              'hdr_gpu_preview_check_system_hdr')
          .asFunction<int Function()>();
    } catch (_) {
      _nativePreviewCheckSystemHdr = () => 0;
    }
    try {
      _nativePreviewCreate = _lib!
          .lookup<NativeFunction<Int32 Function(Int64, Int32, Int32)>>(
              'hdr_gpu_preview_create')
          .asFunction<int Function(int, int, int)>();
    } catch (_) {
      _nativePreviewCreate = (_, _, _) => -1;
    }
    try {
      _nativePreviewSetPosition = _lib!
          .lookup<NativeFunction<Int32 Function(Int32, Int32, Int32, Int32)>>(
              'hdr_gpu_preview_set_position')
          .asFunction<int Function(int, int, int, int)>();
    } catch (_) {
      _nativePreviewSetPosition = (_, _, _, _) => -1;
    }
    try {
      _nativePreviewShow = _lib!
          .lookup<
            NativeFunction<
              Int32 Function(Pointer<Uint8>, Int32, Int32)
            >
          >('hdr_gpu_preview_show')
          .asFunction<int Function(Pointer<Uint8>, int, int)>();
    } catch (_) {
      _nativePreviewShow = (_, _, _) => -1;
    }
    try {
      _nativePreviewHide = _lib!
          .lookup<NativeFunction<Void Function()>>('hdr_gpu_preview_hide')
          .asFunction<void Function()>();
    } catch (_) {
      _nativePreviewHide = () {};
    }
    try {
      _nativePreviewDestroy = _lib!
          .lookup<NativeFunction<Void Function()>>(
              'hdr_gpu_preview_destroy')
          .asFunction<void Function()>();
    } catch (_) {
      _nativePreviewDestroy = () {};
    }
    try {
      _nativePreviewIsHdrAvailable = _lib!
          .lookup<NativeFunction<Int32 Function()>>(
              'hdr_gpu_preview_is_hdr_available')
          .asFunction<int Function()>();
    } catch (_) {
      _nativePreviewIsHdrAvailable = () => 0;
    }
    try {
      _nativePreviewIsVisible = _lib!
          .lookup<NativeFunction<Int32 Function()>>(
              'hdr_gpu_preview_is_visible')
          .asFunction<int Function()>();
    } catch (_) {
      _nativePreviewIsVisible = () => 0;
    }
    try {
      _nativePreviewError = _lib!
          .lookup<NativeFunction<Pointer<Utf8> Function()>>(
              'hdr_gpu_preview_error')
          .asFunction<Pointer<Utf8> Function()>();
    } catch (_) {
      _nativePreviewError = () => nullptr;
    }
  }

  // ========== HDR 预览 ==========

  /// 轻量检测系统是否开启了 HDR (无需创建预览窗口)
  bool get isSystemHdrEnabled {
    if (!_previewLoaded && !loadPreviewOnly()) return false;
    return _nativePreviewCheckSystemHdr() != 0;
  }

  /// HDR 预览是否可用 (显示器支持 HDR, 需先 createPreview)
  bool get isHdrPreviewAvailable => _nativePreviewIsHdrAvailable() != 0;

  /// 预览窗口当前是否可见
  bool get isPreviewVisible => _nativePreviewIsVisible() != 0;

  /// 创建 HDR 预览窗口
  ///
  /// [parentHwnd] 父窗口句柄 (0 自动查找 Flutter 窗口)
  /// [imgWidth], [imgHeight] 预览图像的原始尺寸
  bool createPreview(int parentHwnd, int imgWidth, int imgHeight) {
    final result = _nativePreviewCreate(parentHwnd, imgWidth, imgHeight);
    if (result != 0) {
      final errPtr = _nativePreviewError();
      _lastError = errPtr != nullptr ? errPtr.toDartString() : 'Preview create failed';
      return false;
    }
    return true;
  }

  /// 设置预览窗口位置 (相对于父窗口客户区)
  bool setPreviewPosition(int x, int y, int width, int height) {
    return _nativePreviewSetPosition(x, y, width, height) == 0;
  }

  /// 显示 HDR 预览
  ///
  /// [rgba] RGBA 8-bit 像素数据
  /// [width], [height] 图像尺寸
  bool showPreview(Uint8List rgba, int width, int height) {
    final ptr = calloc<Uint8>(rgba.length);
    try {
      ptr.asTypedList(rgba.length).setAll(0, rgba);
      final result = _nativePreviewShow(ptr, width, height);
      if (result != 0) {
        final errPtr = _nativePreviewError();
        _lastError = errPtr != nullptr ? errPtr.toDartString() : 'Preview show failed';
        return false;
      }
      return true;
    } finally {
      calloc.free(ptr);
    }
  }

  /// 隐藏 HDR 预览
  void hidePreview() {
    _nativePreviewHide();
  }

  /// 销毁 HDR 预览窗口
  void destroyPreview() {
    _nativePreviewDestroy();
  }

  /// 获取上次预览错误
  String? get previewErrorMessage {
    final ptr = _nativePreviewError();
    if (ptr == nullptr) return null;
    final msg = ptr.toDartString();
    return msg.isEmpty ? null : msg;
  }
}
