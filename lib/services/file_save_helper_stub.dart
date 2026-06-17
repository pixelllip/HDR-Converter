import 'dart:typed_data';

/// 将字节数据保存到用户选择的路径
Future<void> saveFile(String path, Uint8List bytes) {
  throw UnsupportedError('当前平台不支持文件保存');
}
