/**
 * ============================================================
 *  HDR 核心代码 — 演示脚本
 *  用于 hdr-demo.html，展示各种 HDR 功能
 * ============================================================
 */

// ===== DOM 引用 =====
const canvas = document.getElementById('hdrCanvas');
const displayArea = document.getElementById('displayArea');
const fileName = document.getElementById('fileName');
const zoomLevel = document.getElementById('zoomLevel');
const statusMessage = document.getElementById('statusMessage');
const statusInfo = document.getElementById('statusInfo');
const hdrIndicator = document.getElementById('hdrIndicator');
const hdrControls = document.getElementById('hdrControls');
const exposureSlider = document.getElementById('exposureSlider');
const exposureValue = document.getElementById('exposureValue');
const toneMapSelect = document.getElementById('toneMapSelect');
const brightnessSlider = document.getElementById('brightnessSlider');
const fileInput = document.getElementById('fileInput');

// ===== 状态 =====
let hdrBuffer = null;
let hdrWidth = 0;
let hdrHeight = 0;
let zoom = 1;

const state = {
  exposure: 0,
  toneMap: 'reinhard',
  brightness: 1,
  colorSpace: 'srgb',
};

// ===== 初始化 =====
state.colorSpace = initCanvasColorSpace(canvas);
const isHdrDisplay = detectHdrDisplay();
hdrIndicator.textContent = isHdrDisplay ? '🔵 HDR' : '⚫ SDR';
hdrIndicator.className = isHdrDisplay ? 'hdr' : 'sdr';

// ===== 渲染 =====
function render() {
  if (!hdrBuffer) return;
  renderHdrToCanvas(hdrBuffer, hdrWidth, hdrHeight, canvas, {
    exposure: state.exposure,
    brightness: state.brightness,
    toneMap: state.toneMap,
    colorSpace: state.colorSpace,
  });
}

// ===== 缩放 =====
function applyZoomStyle() {
  canvas.style.width = (hdrWidth * zoom) + 'px';
  canvas.style.height = (hdrHeight * zoom) + 'px';
  canvas.style.imageRendering = zoom > 2 ? 'pixelated' : 'auto';
  zoomLevel.textContent = Math.round(zoom * 100) + '%';
}

function fitToWindow() {
  if (!hdrWidth || !hdrHeight) return;
  const r = displayArea.getBoundingClientRect();
  zoom = Math.min((r.width - 40) / hdrWidth, (r.height - 40) / hdrHeight, 1);
  applyZoomStyle();
}

function zoomIn() { zoom = Math.min(zoom * 1.25, 10); applyZoomStyle(); }
function zoomOut() { zoom = Math.max(zoom / 1.25, 0.05); applyZoomStyle(); }

// ============================================================
//  生成测试图像
// ============================================================

/**
 * 生成 RGBE 格式的渐变 HDR 图像 (程序生成)
 */
function generateRgbeGradient() {
  const w = 1024, h = 512;
  const pixels = new Float32Array(w * h * 3);

  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      const i = (y * w + x) * 3;
      const u = x / w, v = y / h;

      // 从左到右: 亮度从 0.1 到 10.0
      const luminance = 0.1 + u * 9.9;

      // 颜色: HSL 渐变
      const hue = u * 360;
      const sat = 0.8;
      const lig = 0.5;

      // HSL → RGB (简化)
      const c = (1 - Math.abs(2 * lig - 1)) * sat;
      const hp = hue / 60;
      const x2 = c * (1 - Math.abs(hp % 2 - 1));
      let r1, g1, b1;
      if (hp < 1) { r1 = c; g1 = x2; b1 = 0; }
      else if (hp < 2) { r1 = x2; g1 = c; b1 = 0; }
      else if (hp < 3) { r1 = 0; g1 = c; b1 = x2; }
      else if (hp < 4) { r1 = 0; g1 = x2; b1 = c; }
      else if (hp < 5) { r1 = x2; g1 = 0; b1 = c; }
      else { r1 = c; g1 = 0; b1 = x2; }
      const m = lig - c / 2;

      // 应用 HDR 亮度
      pixels[i]     = (r1 + m) * luminance;
      pixels[i + 1] = (g1 + m) * luminance;
      pixels[i + 2] = (b1 + m) * luminance;
    }
  }

  hdrBuffer = pixels;
  hdrWidth = w;
  hdrHeight = h;
  canvas.width = w;
  canvas.height = h;
  hdrControls.classList.remove('hidden');
  render();
  fitToWindow();
  statusMessage.textContent = '✅ 已生成 RGBE 渐变 (0.1 ~ 10.0 nits)';
  statusInfo.textContent = `${w} × ${h}`;
  fileName.textContent = 'RGBE 渐变测试图';
}

/**
 * 生成 HDR 正弦波测试图案
 */
function generateHdrSineWave() {
  const w = 1024, h = 512;
  const pixels = new Float32Array(w * h * 3);

  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      const i = (y * w + x) * 3;
      const u = x / w, v = y / h;

      // 多频率正弦波叠加，展示色调映射效果
      const freq1 = Math.sin(u * 20) * 0.5 + 0.5;
      const freq2 = Math.sin(u * 5 + v * 3) * 0.3;
      const freq3 = Math.sin((u + v) * 40) * 0.2;

      // 亮度范围: 0.01 ~ 100 nits (5 stops)
      const luminance = Math.pow(10, -2 + freq1 * 4 + freq2 + freq3);

      // 颜色随位置变化
      const r = 0.8 + 0.2 * Math.sin(u * 10);
      const g = 0.8 + 0.2 * Math.sin(v * 10 + 2);
      const b = 0.8 + 0.2 * Math.sin((u + v) * 8 + 4);

      pixels[i]     = r * luminance;
      pixels[i + 1] = g * luminance;
      pixels[i + 2] = b * luminance;
    }
  }

  hdrBuffer = pixels;
  hdrWidth = w;
  hdrHeight = h;
  canvas.width = w;
  canvas.height = h;
  hdrControls.classList.remove('hidden');
  render();
  fitToWindow();
  statusMessage.textContent = '✅ 已生成 HDR 正弦波 (0.01 ~ 100 nits)';
  statusInfo.textContent = `${w} × ${h}`;
  fileName.textContent = 'HDR 正弦波测试图';
}

// ============================================================
//  加载本地 .hdr 文件
// ============================================================

async function loadHdrFile(file) {
  const arrayBuffer = await file.arrayBuffer();
  const uint8 = new Uint8Array(arrayBuffer);

  try {
    const result = decodeRgbe(uint8.buffer);
    hdrBuffer = result.pixels;
    hdrWidth = result.width;
    hdrHeight = result.height;

    canvas.width = hdrWidth;
    canvas.height = hdrHeight;
    hdrControls.classList.remove('hidden');
    render();
    fitToWindow();
    fileName.textContent = file.name;
    statusMessage.textContent = `✅ 已解码: ${file.name}`;
    statusInfo.textContent = `${hdrWidth} × ${hdrHeight}`;
  } catch (e) {
    statusMessage.textContent = `❌ 解码失败: ${e.message}`;
    console.error(e);
  }
}

// ============================================================
//  事件绑定
// ============================================================

// 测试模式
document.querySelectorAll('.test-item').forEach(el => {
  el.addEventListener('click', () => {
    document.querySelectorAll('.test-item').forEach(e => e.classList.remove('active'));
    el.classList.add('active');
    const mode = el.dataset.mode;
    if (mode === 'rgbe') generateRgbeGradient();
    else if (mode === 'hdr_sine') generateHdrSineWave();
    else if (mode === 'load') fileInput.click();
  });
});

// 文件输入
fileInput.addEventListener('change', async (e) => {
  const file = e.target.files?.[0];
  if (!file) return;
  const ext = file.name.split('.').pop().toLowerCase();

  if (ext === 'hdr') {
    await loadHdrFile(file);
  } else {
    // 加载普通图片
    const dataUrl = URL.createObjectURL(file);
    try {
      await loadStandardImage(canvas, dataUrl, state.colorSpace);
      hdrBuffer = null;
      hdrWidth = canvas.width;
      hdrHeight = canvas.height;
      hdrControls.classList.add('hidden');
      fitToWindow();
      fileName.textContent = file.name;
      statusMessage.textContent = `✅ 已加载: ${file.name}`;
      statusInfo.textContent = `${hdrWidth} × ${hdrHeight}`;
    } catch (err) {
      statusMessage.textContent = '❌ 加载失败';
    }
    URL.revokeObjectURL(dataUrl);
  }
  fileInput.value = '';
});

// 工具栏
document.getElementById('btnLoadTest').addEventListener('click', () => fileInput.click());
document.getElementById('btnZoomIn').addEventListener('click', zoomIn);
document.getElementById('btnZoomOut').addEventListener('click', zoomOut);
document.getElementById('btnFit').addEventListener('click', fitToWindow);

// HDR 控件
exposureSlider.addEventListener('input', () => {
  state.exposure = parseFloat(exposureSlider.value);
  exposureValue.textContent = `${state.exposure > 0 ? '+' : ''}${state.exposure.toFixed(1)} EV`;
  if (hdrBuffer) render();
});

toneMapSelect.addEventListener('change', () => {
  state.toneMap = toneMapSelect.value;
  if (hdrBuffer) render();
});

brightnessSlider.addEventListener('input', () => {
  state.brightness = parseFloat(brightnessSlider.value);
  if (hdrBuffer) render();
});

// ===== 窗口缩放 =====
window.addEventListener('resize', fitToWindow);

// ===== 键盘快捷键 =====
document.addEventListener('keydown', (e) => {
  if (e.target.tagName === 'INPUT' || e.target.tagName === 'SELECT') return;
  if ((e.ctrlKey || e.metaKey) && (e.key === '+' || e.key === '=')) { e.preventDefault(); zoomIn(); }
  else if ((e.ctrlKey || e.metaKey) && e.key === '-') { e.preventDefault(); zoomOut(); }
  else if ((e.ctrlKey || e.metaKey) && e.key === '0') { e.preventDefault(); fitToWindow(); }
});

// ===== 拖拽支持 =====
displayArea.addEventListener('dragover', (e) => e.preventDefault());
displayArea.addEventListener('drop', async (e) => {
  e.preventDefault();
  const file = e.dataTransfer.files?.[0];
  if (!file) return;
  if (file.name.toLowerCase().endsWith('.hdr')) {
    await loadHdrFile(file);
  }
});

// ============================================================
//  启动: 默认生成 RGBE 渐变
// ============================================================
generateRgbeGradient();
