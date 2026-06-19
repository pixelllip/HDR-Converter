/// 输出格式
enum OutputFormat {
  hdrPng('HDR PNG (ICC增益)', 'png'),
  ultraHdrJpeg('Ultra HDR JPEG', 'jpg');

  final String label;
  final String value;
  const OutputFormat(this.label, this.value);
}

/// RGB通道独立调整
class RgbChannelAdjustment {
  double red;
  double green;
  double blue;

  RgbChannelAdjustment({this.red = 0.96, this.green = 1.0, this.blue = 1.0});

  RgbChannelAdjustment copy() =>
      RgbChannelAdjustment(red: red, green: green, blue: blue);
}

/// HDR转换设置
class ConversionSettings {
  /// HDR强度 0.96 - 2.00
  double hdrIntensity;

  /// 微调明暗 0.3 - 1.5
  double fineTuneBrightness;

  /// 伽马校正 0.3 - 3.0
  double gamma;

  /// RGB通道调整
  RgbChannelAdjustment rgbAdjustment;

  /// 输出格式
  OutputFormat outputFormat;

  ConversionSettings({
    this.hdrIntensity = 1.18,
    this.fineTuneBrightness = 0.3,
    this.gamma = 0.9,
    RgbChannelAdjustment? rgbAdjustment,
    this.outputFormat = OutputFormat.hdrPng,
  }) : rgbAdjustment = rgbAdjustment ?? RgbChannelAdjustment();

  /// 总曝光强度 = (hdrIntensity * fineTuneBrightness) + 1
  double get totalExposure => (hdrIntensity * fineTuneBrightness) + 1;

  ConversionSettings copy() => ConversionSettings(
    hdrIntensity: hdrIntensity,
    fineTuneBrightness: fineTuneBrightness,
    gamma: gamma,
    rgbAdjustment: rgbAdjustment.copy(),
    outputFormat: outputFormat,
  );
}
