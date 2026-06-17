import 'dart:typed_data';
import 'dart:js_interop';
import 'package:web/web.dart' as web;

/// 根据文件扩展名获取 MIME 类型
String _mimeFromName(String name) {
  final ext = name.split('.').last.toLowerCase();
  switch (ext) {
    case 'png':
      return 'image/png';
    case 'jpg':
    case 'jpeg':
      return 'image/jpeg';
    case 'avif':
      return 'image/avif';
    case 'webp':
      return 'image/webp';
    default:
      return 'application/octet-stream';
  }
}

/// Web 端：通过浏览器下载保存文件
///
/// [fileName] 用作下载文件名。
Future<void> saveFile(String fileName, Uint8List bytes) async {
  final blob = web.Blob(
    [bytes.toJS].toJS,
    web.BlobPropertyBag(type: _mimeFromName(fileName)),
  );
  final url = web.URL.createObjectURL(blob);
  final anchor = web.document.createElement('a') as web.HTMLAnchorElement;
  anchor.href = url;
  anchor.download = fileName;
  anchor.style.display = 'none';
  web.document.body!.appendChild(anchor);
  anchor.click();
  web.URL.revokeObjectURL(url);
  anchor.remove();
}
