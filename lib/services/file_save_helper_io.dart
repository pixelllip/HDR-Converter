import 'dart:io';
import 'dart:typed_data';

/// 将字节数据写入本地文件
Future<void> saveFile(String path, Uint8List bytes) async {
  await File(path).writeAsBytes(bytes);
}
