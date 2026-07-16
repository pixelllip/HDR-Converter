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
}
