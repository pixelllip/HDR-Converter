import 'dart:async';
import 'dart:typed_data';
import 'package:flutter/foundation.dart' show kIsWeb;
import 'package:flutter/material.dart';
import 'package:file_selector/file_selector.dart';
import 'package:path/path.dart' as p;
import '../models/conversion_settings.dart';
import '../services/hdr_converter.dart';
import '../services/file_save_helper.dart';
import '../widgets/settings_panel.dart';

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

  // 输入图像
  Uint8List? _inputBytes;
  String? _inputFileName;

  // 预览图像
  Uint8List? _previewBytes;
  bool _isPreviewLoading = false;
  double _previewProgress = 0.0;

  // 输出
  bool _isExporting = false;
  String? _exportMessage;
  final _fileNameCtrl = TextEditingController(text: 'output');
  String? _saveDir; // 输出目录（Windows 端可自定义）

  // 防抖动计时器
  Timer? _previewDebounce;

  @override
  void initState() {
    super.initState();
    _converter = HdrConverter.instance;
    _initConverter();
  }

  Future<void> _initConverter() async {
    try {
      await _converter.initialize();
    } catch (e) {
      if (mounted) {
        ScaffoldMessenger.of(
          context,
        ).showSnackBar(SnackBar(content: Text('初始化失败: $e')));
      }
    }
  }

  @override
  void dispose() {
    _previewDebounce?.cancel();
    _fileNameCtrl.dispose();
    super.dispose();
  }

  /// 选择输入图像
  Future<void> _pickInputImage() async {
    // 先关闭 HDR 覆盖层（Web 端）
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

    // 先显示原图（立即可见），后台再跑 HDR 处理
    setState(() {
      _inputBytes = bytes;
      _inputFileName = name;
      _previewBytes = bytes; // 临时显示原图
      _isPreviewLoading = false;
      _exportMessage = null;
    });

    _generatePreview();
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
      _syncSettings(settings);
    });
  }

  /// 设置变更结束回调（松开滑块时生成预览）
  void _onSettingsChangeEnd(ConversionSettings settings) {
    setState(() {
      _syncSettings(settings);
    });
    _generatePreview();
  }

  /// 同步设置值
  void _syncSettings(ConversionSettings settings) {
    _settings.hdrIntensity = settings.hdrIntensity;
    _settings.fineTuneBrightness = settings.fineTuneBrightness;
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
      _exportMessage = null;
    });

    try {
      final outputBytes = await _converter.convertSdrToHdr(
        inputBytes: _inputBytes!,
        settings: _settings,
        onProgress: (p) {
          if (mounted) {
            _exportMessage = '处理中 ${(p * 100).toInt()}%';
            setState(() {});
          }
        },
      );

      if (!mounted) return;

      final ext = _converter.getOutputExtension(_settings);
      final baseName = _fileNameCtrl.text.trim();
      final safeName = baseName.isEmpty ? 'output' : baseName;
      final defaultName = '$safeName.$ext';

      // Windows 端有保存目录 → 直接自动保存
      if (!kIsWeb && _saveDir != null) {
        final path = '$_saveDir\\$defaultName';
        _exportMessage = '正在保存...';
        setState(() {});
        await saveFile(path, outputBytes);
        if (mounted) {
          setState(() {
            _exportMessage = '已保存: $defaultName';
            _isExporting = false;
          });
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
        _exportMessage = '正在保存...';
        setState(() {});

        if (kIsWeb) {
          // Web：直接下载
          await saveFile(defaultName, outputBytes);
        } else {
          // Windows：记录目录，用统一路径保存
          _saveDir = p.dirname(destination.path);
          final savePath = '$_saveDir\\$defaultName';
          await saveFile(savePath, outputBytes);
        }

        if (mounted) {
          setState(() {
            _exportMessage = '已保存: $defaultName';
            _isExporting = false;
          });
        }
      } else {
        if (mounted) setState(() => _isExporting = false);
      }
    } catch (e) {
      if (mounted) {
        setState(() {
          _exportMessage = '导出失败: $e';
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
        title: const Text('SDR → HDR 转换'),
        centerTitle: true,
        elevation: 1,
      ),
      body: isWide ? _buildWideLayout() : _buildNarrowLayout(),
    );
  }

  /// 宽屏排版（电脑）：上导入 → 左设置 + 右预览/导出
  Widget _buildWideLayout() {
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

  /// 窄屏排版（手机）：保持原有纵向布局
  Widget _buildNarrowLayout() {
    return Column(
      children: [
        _buildInputSection(),
        Expanded(child: _buildPreviewSection()),
        SettingsPanel(
          settings: _settings.copy(),
          onChanged: _onSettingsChanged,
          onChangeEnd: _onSettingsChangeEnd,
          fileNameController: _fileNameCtrl,
          saveDir: _saveDir,
          onPickSaveDir: kIsWeb ? null : _pickSaveDir,
        ),
        _buildOutputSection(),
      ],
    );
  }

  Widget _buildInputSection() {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 8),
      decoration: BoxDecoration(
        color: Theme.of(context).colorScheme.surfaceContainerLow,
        border: const Border(
          bottom: BorderSide(color: Colors.grey, width: 0.5),
        ),
      ),
      child: Row(
        children: [
          Icon(Icons.image, color: Theme.of(context).colorScheme.primary),
          const SizedBox(width: 8),
          Expanded(
            child: Text(
              _inputFileName ?? '未选择图片',
              overflow: TextOverflow.ellipsis,
              style: TextStyle(
                color: _inputFileName != null ? null : Colors.grey,
              ),
            ),
          ),
          const SizedBox(width: 8),
          FilledButton.tonalIcon(
            onPressed: _pickInputImage,
            icon: const Icon(Icons.folder_open, size: 18),
            label: const Text('打开'),
          ),
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
    return Container(
      padding: const EdgeInsets.all(16),
      decoration: BoxDecoration(
        color: Theme.of(context).colorScheme.surfaceContainerLow,
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
                    LinearProgressIndicator(),
                    const SizedBox(height: 6),
                    Text(
                      _exportMessage ?? '处理中...',
                      style: TextStyle(
                        color: Colors.grey.shade600,
                        fontSize: 12,
                      ),
                    ),
                  ],
                ),
              ),
            // 完成/失败消息
            if (_exportMessage != null && !_isExporting)
              Padding(
                padding: const EdgeInsets.only(bottom: 8),
                child: Text(
                  _exportMessage!,
                  style: TextStyle(
                    color: _exportMessage!.contains('失败')
                        ? Colors.red
                        : Colors.green,
                    fontSize: 13,
                  ),
                  textAlign: TextAlign.center,
                ),
              ),
            Row(
              children: [
                // HDR 预览按钮（仅 Web）
                if (kIsWeb)
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
                      onPressed: (_inputBytes != null && !_isExporting)
                          ? _exportHdr
                          : null,
                      icon: _isExporting
                          ? const SizedBox(
                              width: 18,
                              height: 18,
                              child: CircularProgressIndicator(
                                strokeWidth: 2,
                                color: Colors.white,
                              ),
                            )
                          : const Icon(Icons.save_alt),
                      label: Text(_isExporting ? '导出中...' : '导出 HDR 图像'),
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
