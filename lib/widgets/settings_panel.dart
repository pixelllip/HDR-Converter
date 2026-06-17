import 'package:flutter/material.dart';
import '../models/conversion_settings.dart';

/// 设置面板
class SettingsPanel extends StatelessWidget {
  final ConversionSettings settings;
  final ValueChanged<ConversionSettings> onChanged;
  final ValueChanged<ConversionSettings>? onChangeEnd;
  final TextEditingController? fileNameController;
  final String? saveDir;
  final VoidCallback? onPickSaveDir;

  const SettingsPanel({
    super.key,
    required this.settings,
    required this.onChanged,
    this.onChangeEnd,
    this.fileNameController,
    this.saveDir,
    this.onPickSaveDir,
  });

  @override
  Widget build(BuildContext context) {
    return Container(
      constraints: const BoxConstraints(maxHeight: 280),
      decoration: BoxDecoration(
        color: Theme.of(context).colorScheme.surfaceContainerLow,
        border: const Border(
          top: BorderSide(color: Colors.grey, width: 0.5),
          bottom: BorderSide(color: Colors.grey, width: 0.5),
        ),
      ),
      child: DefaultTabController(
        length: 3,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            TabBar(
              isScrollable: false,
              tabs: const [
                Tab(text: '曝光'),
                Tab(text: 'RGB通道'),
                Tab(text: '输出'),
              ],
            ),
            Flexible(
              child: TabBarView(
                children: [
                  _ExposureTab(
                    settings: settings,
                    onChanged: onChanged,
                    onChangeEnd: onChangeEnd,
                  ),
                  _RgbChannelTab(
                    settings: settings,
                    onChanged: onChanged,
                    onChangeEnd: onChangeEnd,
                  ),
                  _OutputTab(
                    settings: settings,
                    onChanged: onChanged,
                    fileNameController: fileNameController,
                    saveDir: saveDir,
                    onPickSaveDir: onPickSaveDir,
                  ),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// 曝光设置标签页
class _ExposureTab extends StatelessWidget {
  final ConversionSettings settings;
  final ValueChanged<ConversionSettings> onChanged;
  final ValueChanged<ConversionSettings>? onChangeEnd;

  const _ExposureTab({
    required this.settings,
    required this.onChanged,
    this.onChangeEnd,
  });

  @override
  Widget build(BuildContext context) {
    final totalExposure = settings.totalExposure;

    return ListView(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 8),
      children: [
        // HDR 强度
        Row(
          children: [
            const SizedBox(width: 120, child: Text('HDR 强度')),
            Expanded(
              child: Slider(
                value: settings.hdrIntensity,
                min: 0.96,
                max: 2.0,
                divisions: 100,
                label: settings.hdrIntensity.toStringAsFixed(2),
                onChanged: (v) {
                  final updated = settings.copy();
                  updated.hdrIntensity = v;
                  onChanged(updated);
                },
                onChangeEnd: (v) {
                  final updated = settings.copy();
                  updated.hdrIntensity = v;
                  onChangeEnd?.call(updated);
                },
              ),
            ),
            SizedBox(
              width: 40,
              child: Text(
                settings.hdrIntensity.toStringAsFixed(2),
                textAlign: TextAlign.right,
              ),
            ),
          ],
        ),
        // 微调明暗
        Row(
          children: [
            const SizedBox(width: 120, child: Text('微调明暗')),
            Expanded(
              child: Slider(
                value: settings.fineTuneBrightness,
                min: 0.3,
                max: 1.5,
                divisions: 140,
                label: settings.fineTuneBrightness.toStringAsFixed(2),
                onChanged: (v) {
                  final updated = settings.copy();
                  updated.fineTuneBrightness = v;
                  onChanged(updated);
                },
                onChangeEnd: (v) {
                  final updated = settings.copy();
                  updated.fineTuneBrightness = v;
                  onChangeEnd?.call(updated);
                },
              ),
            ),
            SizedBox(
              width: 40,
              child: Text(
                settings.fineTuneBrightness.toStringAsFixed(2),
                textAlign: TextAlign.right,
              ),
            ),
          ],
        ),
        const Divider(height: 16),
        // 总曝光强度
        Row(
          children: [
            const SizedBox(width: 120, child: Text('总曝光强度')),
            Text(
              '×${totalExposure.toStringAsFixed(3)}',
              style: Theme.of(context).textTheme.titleMedium?.copyWith(
                fontWeight: FontWeight.bold,
                color: Theme.of(context).colorScheme.primary,
              ),
            ),
          ],
        ),
        Text(
          '总曝光 = (HDR强度 × 微调明暗) + 1',
          style: Theme.of(
            context,
          ).textTheme.bodySmall?.copyWith(color: Colors.grey),
        ),
      ],
    );
  }
}

/// RGB 通道调整标签页
class _RgbChannelTab extends StatelessWidget {
  final ConversionSettings settings;
  final ValueChanged<ConversionSettings> onChanged;
  final ValueChanged<ConversionSettings>? onChangeEnd;

  const _RgbChannelTab({
    required this.settings,
    required this.onChanged,
    this.onChangeEnd,
  });

  @override
  Widget build(BuildContext context) {
    return ListView(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 8),
      children: [
        // 红色通道
        _ChannelSlider(
          label: '红色 (R)',
          color: Colors.red,
          value: settings.rgbAdjustment.red,
          onChanged: (v) {
            final updated = settings.copy();
            updated.rgbAdjustment.red = v;
            onChanged(updated);
          },
          onChangeEnd: (v) {
            final updated = settings.copy();
            updated.rgbAdjustment.red = v;
            onChangeEnd?.call(updated);
          },
        ),
        // 绿色通道
        _ChannelSlider(
          label: '绿色 (G)',
          color: Colors.green,
          value: settings.rgbAdjustment.green,
          onChanged: (v) {
            final updated = settings.copy();
            updated.rgbAdjustment.green = v;
            onChanged(updated);
          },
          onChangeEnd: (v) {
            final updated = settings.copy();
            updated.rgbAdjustment.green = v;
            onChangeEnd?.call(updated);
          },
        ),
        // 蓝色通道
        _ChannelSlider(
          label: '蓝色 (B)',
          color: Colors.blue,
          value: settings.rgbAdjustment.blue,
          onChanged: (v) {
            final updated = settings.copy();
            updated.rgbAdjustment.blue = v;
            onChanged(updated);
          },
          onChangeEnd: (v) {
            final updated = settings.copy();
            updated.rgbAdjustment.blue = v;
            onChangeEnd?.call(updated);
          },
        ),
        const SizedBox(height: 8),
        // 重置按钮
        Center(
          child: TextButton.icon(
            onPressed: () {
              final updated = settings.copy();
              updated.rgbAdjustment.red = 1.0;
              updated.rgbAdjustment.green = 1.0;
              updated.rgbAdjustment.blue = 1.0;
              onChanged(updated);
            },
            icon: const Icon(Icons.restart_alt, size: 16),
            label: const Text('重置通道'),
          ),
        ),
      ],
    );
  }
}

/// 通道滑块组件
class _ChannelSlider extends StatelessWidget {
  final String label;
  final Color color;
  final double value;
  final ValueChanged<double> onChanged;
  final ValueChanged<double>? onChangeEnd;

  const _ChannelSlider({
    required this.label,
    required this.color,
    required this.value,
    required this.onChanged,
    this.onChangeEnd,
  });

  @override
  Widget build(BuildContext context) {
    return Row(
      children: [
        Container(
          width: 70,
          alignment: Alignment.centerLeft,
          child: Text(label, style: TextStyle(color: color)),
        ),
        Expanded(
          child: Slider(
            value: value,
            min: 0.0,
            max: 3.0,
            divisions: 300,
            activeColor: color,
            label: value.toStringAsFixed(2),
            onChanged: onChanged,
            onChangeEnd: onChangeEnd ?? onChanged,
          ),
        ),
        SizedBox(
          width: 40,
          child: Text(value.toStringAsFixed(2), textAlign: TextAlign.right),
        ),
      ],
    );
  }
}

/// 输出设置标签页
class _OutputTab extends StatelessWidget {
  final ConversionSettings settings;
  final ValueChanged<ConversionSettings> onChanged;
  final TextEditingController? fileNameController;
  final String? saveDir;
  final VoidCallback? onPickSaveDir;

  const _OutputTab({
    required this.settings,
    required this.onChanged,
    this.fileNameController,
    this.saveDir,
    this.onPickSaveDir,
  });

  @override
  Widget build(BuildContext context) {
    final ext = settings.outputFormat.value;

    return ListView(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 8),
      children: [
        // 输出文件名
        const Text('输出文件名'),
        const SizedBox(height: 4),
        Row(
          children: [
            Expanded(
              child: TextField(
                controller: fileNameController,
                decoration: const InputDecoration(
                  isDense: true,
                  contentPadding: EdgeInsets.symmetric(
                    horizontal: 12,
                    vertical: 8,
                  ),
                  border: OutlineInputBorder(),
                ),
                style: const TextStyle(fontSize: 14),
              ),
            ),
            const SizedBox(width: 4),
            Container(
              padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 8),
              decoration: BoxDecoration(
                border: Border.all(color: Colors.grey.shade400),
                borderRadius: BorderRadius.circular(4),
              ),
              child: Text(
                '.$ext',
                style: TextStyle(fontSize: 14, color: Colors.grey.shade600),
              ),
            ),
          ],
        ),
        const SizedBox(height: 12),
        // 输出位置（仅桌面端）
        if (onPickSaveDir != null) ...[
          const Text('输出位置'),
          const SizedBox(height: 4),
          Row(
            children: [
              Expanded(
                child: Container(
                  padding: const EdgeInsets.symmetric(
                    horizontal: 12,
                    vertical: 8,
                  ),
                  decoration: BoxDecoration(
                    border: Border.all(color: Colors.grey.shade400),
                    borderRadius: BorderRadius.circular(4),
                  ),
                  child: Text(
                    saveDir ?? '未设置（导出时选择）',
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      fontSize: 13,
                      color: saveDir != null ? null : Colors.grey,
                    ),
                  ),
                ),
              ),
              const SizedBox(width: 4),
              IconButton(
                onPressed: onPickSaveDir,
                icon: const Icon(Icons.folder_open, size: 20),
                tooltip: '选择输出目录',
                visualDensity: VisualDensity.compact,
              ),
            ],
          ),
          const SizedBox(height: 12),
        ],
        // 输出格式
        const Text('输出格式'),
        const SizedBox(height: 4),
        SegmentedButton<OutputFormat>(
          segments: const [
            ButtonSegment(
              value: OutputFormat.hdrPng,
              label: Text('ICC增益'),
              tooltip: '8位PNG + BT.2020 ICC，兼容性最佳',
            ),
            ButtonSegment(
              value: OutputFormat.avif,
              label: Text('AVIF'),
              tooltip: '高效压缩格式',
            ),
            ButtonSegment(
              value: OutputFormat.ultraHdrJpeg,
              label: Text('Ultra HDR JPEG'),
              tooltip: '兼容标准JPEG查看器',
            ),
          ],
          selected: {settings.outputFormat},
          onSelectionChanged: (v) {
            final updated = settings.copy();
            updated.outputFormat = v.first;
            onChanged(updated);
          },
        ),
        const SizedBox(height: 8),
        Text(
          _getFormatDescription(settings.outputFormat),
          style: Theme.of(
            context,
          ).textTheme.bodySmall?.copyWith(color: Colors.grey.shade600),
        ),
      ],
    );
  }

  String _getFormatDescription(OutputFormat format) {
    switch (format) {
      case OutputFormat.hdrPng:
        return '8位 PNG + BT.2020 ICC，ICC增益技术，广泛支持';
      case OutputFormat.avif:
        return 'AVIF 格式，支持 HDR，压缩率更高';
      case OutputFormat.ultraHdrJpeg:
        return 'Ultra HDR JPEG，向后兼容标准 JPEG';
    }
  }
}
