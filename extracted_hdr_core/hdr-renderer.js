/**
 * ============================================================
 *  HDR 渲染器 — Canvas 初始化、图像加载、缩放控制
 *  依赖 hdr-core.js
 * ============================================================
 */

// ============================================================
//  HDR 显示检测
// ============================================================

/**
 * 检测当前显示器是否支持高动态范围 (HDR)
 * @returns {boolean}
 */
function detectHdrDisplay() {
  return window.matchMedia('(dynamic-range: high)').matches;
}

// ============================================================
//  Canvas 色彩空间初始化
// ============================================================

/**
 * 初始化 Canvas，检测是否支持 display-p3 色彩空间
 *
 * @param {HTMLCanvasElement} canvas
 * @returns {string} 检测到的色彩空间 ('display-p3' | 'srgb')
 */
function initCanvasColorSpace(canvas) {
  try {
    const tempCtx = canvas.getContext('2d', { colorSpace: 'display-p3' });
    if (tempCtx && tempCtx.colorSpace === 'display-p3') {
      return 'display-p3';
    }
  } catch (e) { /* fallback */ }
  return 'srgb';
}

/**
 * 获取指定色彩空间的 Canvas 上下文
 *
 * @param {HTMLCanvasElement} canvas
 * @param {string} colorSpace - 'srgb' | 'display-p3'
 * @returns {CanvasRenderingContext2D}
 */
function getCanvasCtx(canvas, colorSpace) {
  return canvas.getContext('2d', { colorSpace });
}

// ============================================================
//  图像加载函数
// ============================================================

/**
 * 加载普通 SDR 图片到 Canvas
 *
 * @param {HTMLCanvasElement} canvas - 目标 Canvas
 * @param {string} dataUrl - 图片的 data: URL
 * @param {string} colorSpace - Canvas 色彩空间
 * @returns {Promise<HTMLImageElement>}
 */
function loadStandardImage(canvas, dataUrl, colorSpace = 'srgb') {
  return new Promise((resolve, reject) => {
    const img = new Image();
    img.onload = () => {
      canvas.width = img.naturalWidth;
      canvas.height = img.naturalHeight;
      const ctx = getCanvasCtx(canvas, colorSpace);
      ctx.drawImage(img, 0, 0);
      resolve(img);
    };
    img.onerror = reject;
    img.src = dataUrl;
  });
}

/**
 * 加载 ICC HDR 图片 (使用 img 标签，浏览器原生色彩管理)
 *
 * @param {HTMLImageElement} imgElement - 用于显示的 img 元素
 * @param {string} dataUrl - 图片的 data: URL
 * @returns {Promise<HTMLImageElement>}
 */
function loadIccHdrImage(imgElement, dataUrl) {
  return new Promise((resolve, reject) => {
    imgElement.onload = () => resolve(imgElement);
    imgElement.onerror = reject;
    imgElement.src = dataUrl;
  });
}

/**
 * 加载 RGBE (.hdr) 格式图片
 *
 * @param {Uint8Array} binaryData - .hdr 文件的原始二进制数据
 * @returns {{ pixels: Float32Array, width: number, height: number }}
 */
function loadHdrRgbe(binaryData) {
  return decodeRgbe(binaryData.buffer);
}

/**
 * 加载 Gain Map (Ultra HDR) 图片
 *
 * @param {HTMLCanvasElement} canvas - 用于显示的 Canvas
 * @param {string} baseDataUrl - SDR 基础图像的 data: URL
 * @param {string} gainMapDataUrl - Gain Map 图像的 data: URL
 * @param {number} backlight - 最大背光倍率
 * @param {function} onHdrBuffer - 回调: (Float32Array hdrBuffer, width, height) => void
 * @param {string} colorSpace - Canvas 色彩空间
 */
async function loadGainMapImage(canvas, baseDataUrl, gainMapDataUrl, backlight, onHdrBuffer, colorSpace = 'srgb') {
  // 1. 加载基础 SDR 图像
  const baseImg = new Image();
  await new Promise((res, rej) => {
    baseImg.onload = res;
    baseImg.onerror = rej;
    baseImg.src = baseDataUrl;
  });

  // 2. 加载 Gain Map 图像
  const gainImg = new Image();
  await new Promise((res, rej) => {
    gainImg.onload = res;
    gainImg.onerror = rej;
    gainImg.src = gainMapDataUrl;
  });

  if (!gainImg.naturalWidth) {
    throw new Error('Gain Map 图像无效');
  }

  // 3. 读取基础图像到线性缓冲区
  const tempCanvas = document.createElement('canvas');
  tempCanvas.width = baseImg.naturalWidth;
  tempCanvas.height = baseImg.naturalHeight;
  tempCanvas.getContext('2d').drawImage(baseImg, 0, 0);
  const sdrData = readCanvasPixelsLinear(tempCanvas);

  // 4. 读取 Gain Map
  const gainData = readGainMapPixels(gainImg, backlight);

  // 5. 重建 HDR
  const hdrBuffer = applyGainMap(sdrData, gainData);

  // 6. 设置 Canvas
  canvas.width = sdrData.width;
  canvas.height = sdrData.height;

  if (onHdrBuffer) {
    onHdrBuffer(hdrBuffer, sdrData.width, sdrData.height);
  }

  return { hdrBuffer, width: sdrData.width, height: sdrData.height };
}

// ============================================================
//  缩放控制
// ============================================================

/**
 * 计算适应窗口的缩放比例
 *
 * @param {number} imgW - 图像宽度
 * @param {number} imgH - 图像高度
 * @param {number} containerW - 容器宽度
 * @param {number} containerH - 容器高度
 * @param {number} [maxZoom=1] - 最大缩放比例(默认1=不超过原始大小)
 * @returns {number}
 */
function calcFitZoom(imgW, imgH, containerW, containerH, maxZoom = 1) {
  return Math.min((containerW - 40) / imgW, (containerH - 40) / imgH, maxZoom);
}

/**
 * 应用缩放变换
 *
 * @param {HTMLCanvasElement|HTMLImageElement} element - Canvas 或 img 元素
 * @param {number} zoom - 缩放比例
 * @param {number} naturalWidth - 原始宽度
 * @param {number} naturalHeight - 原始高度
 */
function applyZoom(element, zoom, naturalWidth, naturalHeight) {
  if (element instanceof HTMLCanvasElement) {
    element.style.width = (naturalWidth * zoom) + 'px';
    element.style.height = (naturalHeight * zoom) + 'px';
    element.style.imageRendering = zoom > 2 ? 'pixelated' : 'auto';
  } else {
    element.style.width = (naturalWidth * zoom) + 'px';
    element.style.height = (naturalHeight * zoom) + 'px';
  }
}

// ============================================================
//  导出
// ============================================================
if (typeof module !== 'undefined' && module.exports) {
  module.exports = {
    detectHdrDisplay,
    initCanvasColorSpace,
    getCanvasCtx,
    loadStandardImage,
    loadIccHdrImage,
    loadHdrRgbe,
    loadGainMapImage,
    calcFitZoom,
    applyZoom,
  };
}
