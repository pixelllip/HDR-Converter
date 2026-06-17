// =====================================================================
// Web HDR 预览组件
//
// 使用 `package:web` 的 DOM API 在页面内创建全屏覆盖层，
// 用浏览器原生 <img> 标签显示带 ICC 配置的 16-bit PNG。
// 浏览器完整支持 ICC 色彩管理，HDR 屏上自动呈现正确色彩。
//
// 此文件仅在 Web 平台编译（条件导出 dart.library.js）。
// =====================================================================
import 'dart:convert';
import 'dart:js_interop';
import 'dart:typed_data';
import 'package:web/web.dart' as web;

/// 将 PNG 字节转为 data URI
String pngBytesToDataUri(Uint8List bytes) {
  return 'data:image/png;base64,${base64Encode(bytes)}';
}

/// 关闭 HDR 预览覆盖层
void dismissHdrPreview() {
  final existing = web.document.getElementById('hdr_overlay');
  if (existing != null) existing.remove();
}

/// 在页面内显示 HDR 预览覆盖层
///
/// 使用 [package:web] 创建全屏暗色覆盖层 + 原生 <img> 标签，
/// 浏览器 ICC 色彩管理自动生效，HDR 屏上呈现正确色彩。
/// 点击覆盖层或按 ESC 键关闭。
void showHdrPreviewOverlay(Uint8List pngBytes) {
  final dataUri = pngBytesToDataUri(pngBytes);

  // 移除已有覆盖层
  final existing = web.document.getElementById('hdr_overlay');
  if (existing != null) existing.remove();

  // 创建覆盖层
  final overlay = web.HTMLDivElement();
  overlay.id = 'hdr_overlay';
  overlay.style
    ..position = 'fixed'
    ..top = '0'
    ..left = '0'
    ..width = '100vw'
    ..height = '100vh'
    ..backgroundColor = 'rgba(0,0,0,0.92)'
    ..display = 'flex'
    ..alignItems = 'center'
    ..justifyContent = 'center'
    ..zIndex = '99999';

  // 关闭按钮
  final closeBtn = web.HTMLDivElement();
  closeBtn.textContent = '✕';
  closeBtn.style
    ..position = 'absolute'
    ..top = '16px'
    ..right = '20px'
    ..color = 'white'
    ..fontSize = '32px'
    ..fontFamily = 'sans-serif'
    ..cursor = 'pointer'
    ..zIndex = '100000'
    ..userSelect = 'none';
  overlay.append(closeBtn);

  // HDR 图片
  final img = web.HTMLImageElement();
  img.src = dataUri;
  img.style
    ..maxWidth = '95vw'
    ..maxHeight = '95vh'
    ..objectFit = 'contain'
    ..borderRadius = '4px';
  overlay.append(img);

  // 添加到页面
  web.document.body!.append(overlay);

  // 关闭逻辑
  void close() => overlay.remove();

  overlay.addEventListener(
    'click',
    (web.Event _) {
      close();
    }.toJS,
  );

  closeBtn.addEventListener(
    'click',
    (web.Event e) {
      e.stopPropagation();
      close();
    }.toJS,
  );

  web.document.addEventListener(
    'keydown',
    (web.Event e) {
      if (e.isA<web.KeyboardEvent>() &&
          (e as web.KeyboardEvent).key == 'Escape') {
        close();
      }
    }.toJS,
  );
}
