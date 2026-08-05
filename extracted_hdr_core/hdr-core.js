/**
 * ============================================================
 *  HDR Core — HDR 图片显示核心数学与处理逻辑
 *  无任何外部依赖，纯 JavaScript
 * ============================================================
 */

// ============================================================
//  sRGB <-> Linear 转换
// ============================================================

/**
 * sRGB 非线性值 → 线性光强
 */
function srgbToLinear(v) {
  return v <= 0.04045 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4);
}

/**
 * 线性光强 → sRGB 非线性值
 */
function linearToSrgb(v) {
  return v <= 0.0031308 ? 12.92 * v : 1.055 * Math.pow(v, 1 / 2.2) - 0.055;
}

// ============================================================
//  曝光控制
// ============================================================

/**
 * 应用曝光补偿
 * @param {number} val - 像素值
 * @param {number} ev - 曝光值 (EV)，每 +1 亮度翻倍
 * @returns {number}
 */
function applyExposure(val, ev) {
  return val * Math.pow(2, ev);
}

// ============================================================
//  色调映射 (Tone Mapping)
// ============================================================

/**
 * Reinhard 色调映射: v / (1 + v)
 * 简单、稳定，适合大多数 HDR 内容
 */
function reinhardToneMap(v) {
  return v / (1 + v);
}

/**
 * ACES 色调映射 (Academy Color Encoding System)
 * 电影级色调映射，暗部更饱和，高光柔和
 */
function acesToneMap(v) {
  return (v * (2.51 * v + 0.03)) / (v * (2.43 * v + 0.59) + 0.14);
}

/**
 * Filmic (ALU) 色调映射
 * 类似 Unreal Engine 的 Filmic 映射，对比度更高
 */
function filmicToneMap(v) {
  const x = Math.max(0, v - 0.004);
  return (x * (6.2 * x + 0.5)) / (x * (6.2 * x + 1.7) + 0.06);
}

/**
 * Gamma 校正 (sRGB gamma)
 */
function gammaCorrection(v) {
  return v <= 0.0031308 ? 12.92 * v : 1.055 * Math.pow(v, 1 / 2.2) - 0.055;
}

/**
 * 根据名称获取色调映射函数
 * @param {string} name - 'reinhard' | 'aces' | 'filmic' | 'none'
 * @returns {function}
 */
function getToneMapFn(name) {
  switch (name) {
    case 'reinhard': return reinhardToneMap;
    case 'aces':     return acesToneMap;
    case 'filmic':   return filmicToneMap;
    case 'none':
    default:         return (v) => Math.min(v, 1);
  }
}

// ============================================================
//  RGBE (.hdr) 解码器
// ============================================================

/**
 * 解码 Radiance RGBE (.hdr) 格式
 *
 * @param {ArrayBuffer} data - .hdr 文件的原始二进制数据
 * @returns {{ pixels: Float32Array, width: number, height: number }}
 *   pixels 为 [R,G,B, R,G,B, ...] 线性浮点数据
 *
 * 支持:
 * - 新风格 RLE (扫描线开头为 2)
 * - 旧风格 RLE (扫描线开头字节 >= 128)
 * - 未压缩格式
 */
function decodeRgbe(data) {
  const view = new DataView(data);
  let offset = 0;
  let header = '';

  // 1. 解析头信息 — 读取直到空行
  while (offset < data.byteLength) {
    const byte = view.getUint8(offset);
    header += String.fromCharCode(byte);
    offset++;
    if (header.endsWith('\n\n') || header.endsWith('\r\n\r\n')) break;
  }

  // 2. 提取分辨率: -Y <height> +X <width>
  const resMatch = header.match(/-Y\s+(\d+)\s+\+X\s+(\d+)/);
  if (!resMatch) throw new Error('无法解析 HDR 分辨率');
  const height = parseInt(resMatch[1]);
  const width = parseInt(resMatch[2]);
  const totalPixels = width * height;
  const pixels = new Float32Array(totalPixels * 3);
  let idx = 0;

  // 3. 逐行解码像素
  for (let y = 0; y < height; y++) {
    if (offset + 4 > data.byteLength) break;

    const r0 = view.getUint8(offset);
    const g0 = view.getUint8(offset + 1);
    const b0 = view.getUint8(offset + 2);
    const e0 = view.getUint8(offset + 3);

    // ----- 新风格 RLE (扫描线起始标记 2) -----
    if (r0 === 2) {
      offset += 4;
      const channels = [];
      for (let c = 0; c < 4; c++) {
        channels[c] = [];
        let pos = 0;
        while (pos < width) {
          if (offset >= data.byteLength) break;
          const code = view.getUint8(offset++);
          if (code > 128) {
            // RLE: 重复 count 次
            const count = code - 128;
            if (offset >= data.byteLength) break;
            const val = view.getUint8(offset++);
            for (let i = 0; i < count && pos < width; i++) {
              channels[c].push(val);
              pos++;
            }
          } else {
            // 原始数据: 读取 code 个字节
            for (let i = 0; i < code && pos < width; i++) {
              if (offset >= data.byteLength) break;
              channels[c].push(view.getUint8(offset++));
              pos++;
            }
          }
        }
      }
      // 组合 RGBE 通道 → 浮点 RGB
      for (let x = 0; x < width && idx < totalPixels; x++) {
        const r = channels[0]?.[x] || 0;
        const g = channels[1]?.[x] || 0;
        const b = channels[2]?.[x] || 0;
        const e = channels[3]?.[x] || 0;
        const exp = Math.pow(2, e - 128 - 8);
        pixels[idx * 3]     = (r + 0.5) * exp;
        pixels[idx * 3 + 1] = (g + 0.5) * exp;
        pixels[idx * 3 + 2] = (b + 0.5) * exp;
        idx++;
      }
      continue;
    }

    // ----- 旧风格 RLE (首字节 >= 128) -----
    if (r0 >= 128) {
      offset += 4;
      const scanline = new Uint8Array(width * 4);
      for (let c = 0; c < 4; c++) {
        if (offset >= data.byteLength) break;
        const code = view.getUint8(offset++);
        const count = code - 128;
        const val = view.getUint8(offset++);
        for (let i = 0; i < count && i < width; i++) {
          scanline[i * 4 + c] = val;
        }
      }
      for (let x = 0; x < width && idx < totalPixels; x++) {
        const exp = Math.pow(2, scanline[x * 4 + 3] - 128 - 8);
        pixels[idx * 3]     = (scanline[x * 4]     + 0.5) * exp;
        pixels[idx * 3 + 1] = (scanline[x * 4 + 1] + 0.5) * exp;
        pixels[idx * 3 + 2] = (scanline[x * 4 + 2] + 0.5) * exp;
        idx++;
      }
    } else {
      // ----- 未压缩格式 -----
      for (let x = 0; x < width && idx < totalPixels; x++) {
        if (offset + 4 > data.byteLength) break;
        const r = view.getUint8(offset++);
        const g = view.getUint8(offset++);
        const b = view.getUint8(offset++);
        const e = view.getUint8(offset++);
        const exp = Math.pow(2, e - 128 - 8);
        pixels[idx * 3]     = (r + 0.5) * exp;
        pixels[idx * 3 + 1] = (g + 0.5) * exp;
        pixels[idx * 3 + 2] = (b + 0.5) * exp;
        idx++;
      }
    }
  }

  return { pixels, width, height };
}

// ============================================================
//  HDR Canvas 渲染
// ============================================================

/**
 * 将 HDR 线性浮点缓冲区渲染到 Canvas
 *
 * @param {Float32Array} hdrBuffer - [R,G,B, R,G,B, ...] 线性浮点数据
 * @param {number} width - 图像宽度
 * @param {number} height - 图像高度
 * @param {HTMLCanvasElement} canvas - 目标 canvas
 * @param {object} options - 渲染选项
 * @param {number} [options.exposure=0] - 曝光补偿 (EV)
 * @param {number} [options.brightness=1] - 亮度倍率
 * @param {string} [options.toneMap='reinhard'] - 色调映射模式
 * @param {string} [options.colorSpace='srgb'] - Canvas 色彩空间 ('srgb' | 'display-p3')
 */
function renderHdrToCanvas(hdrBuffer, width, height, canvas, options = {}) {
  if (!hdrBuffer || width === 0 || height === 0) return;

  const {
    exposure = 0,
    brightness = 1,
    toneMap = 'reinhard',
    colorSpace = 'srgb',
  } = options;

  // 获取 Canvas 上下文 (支持 display-p3)
  let ctx;
  try {
    ctx = canvas.getContext('2d', { colorSpace });
  } catch (e) {
    ctx = canvas.getContext('2d');
  }

  canvas.width = width;
  canvas.height = height;

  const imageData = ctx.createImageData(width, height);
  const d = imageData.data;
  const tmFn = getToneMapFn(toneMap);

  for (let i = 0; i < hdrBuffer.length / 3; i++) {
    let r = hdrBuffer[i * 3];
    let g = hdrBuffer[i * 3 + 1];
    let b = hdrBuffer[i * 3 + 2];

    // 曝光 & 亮度
    r = applyExposure(r, exposure) * brightness;
    g = applyExposure(g, exposure) * brightness;
    b = applyExposure(b, exposure) * brightness;

    // 色调映射 → Gamma 校正 → 钳位
    r = Math.max(0, Math.min(1, gammaCorrection(tmFn(r))));
    g = Math.max(0, Math.min(1, gammaCorrection(tmFn(g))));
    b = Math.max(0, Math.min(1, gammaCorrection(tmFn(b))));

    d[i * 4]     = Math.round(r * 255);
    d[i * 4 + 1] = Math.round(g * 255);
    d[i * 4 + 2] = Math.round(b * 255);
    d[i * 4 + 3] = 255;
  }

  ctx.putImageData(imageData, 0, 0);
}

// ============================================================
//  Gain Map 重建
// ============================================================

/**
 * 从 Canvas 读取像素数据到线性 Float32Array
 * @param {HTMLCanvasElement} canvas
 * @returns {{ pixels: Float32Array, width: number, height: number }}
 */
function readCanvasPixelsLinear(canvas) {
  const ctx = canvas.getContext('2d', { colorSpace: 'srgb' });
  const w = canvas.width, h = canvas.height;
  const imageData = ctx.getImageData(0, 0, w, h);
  const d = imageData.data;
  const buf = new Float32Array(w * h * 3);
  for (let i = 0; i < w * h; i++) {
    buf[i * 3]     = srgbToLinear(d[i * 4]     / 255);
    buf[i * 3 + 1] = srgbToLinear(d[i * 4 + 1] / 255);
    buf[i * 3 + 2] = srgbToLinear(d[i * 4 + 2] / 255);
  }
  return { pixels: buf, width: w, height: h };
}

/**
 * 读取 Gain Map 图像并转换为 recovery 值数组（0..1）
 *
 * @param {HTMLImageElement} gainImg - Gain Map 图像（8-bit 灰度或 RGB）
 * @returns {{ pixels: Float32Array, width: number, height: number }}
 *   pixels 为每个像素的 recovery 值（encoded/255，0..1）
 */
function readGainMapPixels(gainImg) {
  const w = gainImg.naturalWidth, h = gainImg.naturalHeight;
  const cvs = document.createElement('canvas');
  cvs.width = w; cvs.height = h;
  const ctx = cvs.getContext('2d');
  ctx.drawImage(gainImg, 0, 0);
  const imageData = ctx.getImageData(0, 0, w, h);
  const d = imageData.data;
  const gainMap = new Float32Array(w * h);

  for (let i = 0; i < w * h; i++) {
    // 增益图为 8-bit；取灰度作为 recovery = encoded/255
    const gray = 0.2126 * (d[i * 4] / 255)
               + 0.7152 * (d[i * 4 + 1] / 255)
               + 0.0722 * (d[i * 4 + 2] / 255);
    gainMap[i] = gray;
  }
  return { pixels: gainMap, width: w, height: h };
}

/**
 * 应用 Gain Map 重建 HDR 线性缓冲区（Ultra HDR 规范公式）
 *
 *   recovery      = encoded / 255
 *   log_recovery  = recovery^(1/map_gamma)
 *   log_boost     = GainMapMin*(1-log_recovery) + GainMapMax*log_recovery
 *   weight        = clamp((log2(max_display_boost)-HDRCapacityMin)
 *                        /(HDRCapacityMax-HDRCapacityMin), 0, 1)
 *   HDR(x,y)      = (SDR(x,y)+offset_sdr)*2^(log_boost*weight) - offset_hdr
 *
 * @param {{pixels:Float32Array,width:number,height:number}} sdr - SDR 线性基础图像
 * @param {{pixels:Float32Array,width:number,height:number}} gainMap - recovery 增益图
 * @param {object} [metadata] - hdrgm 元数据
 * @param {number} [maxDisplayBoost] - 显示器当前最大提升倍数
 * @returns {Float32Array} HDR 线性浮点缓冲 [R,G,B, ...]
 */
function applyGainMap(sdr, gainMap, metadata = {}, maxDisplayBoost = 4.0) {
  const gainMapMin = metadata.gainMapMinLog2 != null ? metadata.gainMapMinLog2 : 0;
  const gainMapMax = metadata.gainMapMaxLog2 != null ? metadata.gainMapMaxLog2 : Math.log2(4.0);
  const gamma = metadata.gamma != null ? metadata.gamma : 1;
  const offsetSdr = metadata.offsetSdr != null ? metadata.offsetSdr : 1 / 64;
  const offsetHdr = metadata.offsetHdr != null ? metadata.offsetHdr : 1 / 64;
  const hdrCapMin = metadata.hdrCapacityMinLog2 != null ? metadata.hdrCapacityMinLog2 : 0;
  const hdrCapMax = metadata.hdrCapacityMaxLog2 != null ? metadata.hdrCapacityMaxLog2 : gainMapMax;
  const baseIsHdr = !!metadata.baseRenditionIsHdr;

  // 显示提升权重（SDR 为主图像的情况）
  let unclampedWeight = (Math.log2(maxDisplayBoost) - hdrCapMin) / (hdrCapMax - hdrCapMin);
  let weight = Math.max(0, Math.min(1, unclampedWeight));
  if (baseIsHdr) weight = 1 - weight;

  const total = sdr.width * sdr.height;
  const hdr = new Float32Array(total * 3);
  const scaleX = sdr.width / gainMap.width;
  const scaleY = sdr.height / gainMap.height;

  for (let y = 0; y < sdr.height; y++) {
    const gyf = y * scaleY;
    const gy0 = Math.min(Math.floor(gyf), gainMap.height - 1);
    const gy1 = Math.min(gy0 + 1, gainMap.height - 1);
    const fy = gyf - gy0;
    for (let x = 0; x < sdr.width; x++) {
      const si = (y * sdr.width + x) * 3;
      const gxf = x * scaleX;
      const gx0 = Math.min(Math.floor(gxf), gainMap.width - 1);
      const gx1 = Math.min(gx0 + 1, gainMap.width - 1);
      const fx = gxf - gx0;

      // 双线性采样 recovery
      const g00 = gainMap.pixels[gy0 * gainMap.width + gx0];
      const g01 = gainMap.pixels[gy0 * gainMap.width + gx1];
      const g10 = gainMap.pixels[gy1 * gainMap.width + gx0];
      const g11 = gainMap.pixels[gy1 * gainMap.width + gx1];
      const recovery = (g00 * (1 - fx) + g01 * fx) * (1 - fy) + (g10 * (1 - fx) + g11 * fx) * fy;

      // 规范公式
      const logRecovery = Math.pow(recovery, 1 / gamma);
      const logBoost = gainMapMin * (1 - logRecovery) + gainMapMax * logRecovery;
      const gainFactor = Math.pow(2, logBoost * weight);

      hdr[si]     = (sdr.pixels[si]     + offsetSdr) * gainFactor - offsetHdr;
      hdr[si + 1] = (sdr.pixels[si + 1] + offsetSdr) * gainFactor - offsetHdr;
      hdr[si + 2] = (sdr.pixels[si + 2] + offsetSdr) * gainFactor - offsetHdr;
    }
  }
  return hdr;
}

// ============================================================
//  导出 (支持 ES Module / CommonJS / 浏览器全局)
// ============================================================
if (typeof module !== 'undefined' && module.exports) {
  module.exports = {
    srgbToLinear, linearToSrgb,
    applyExposure,
    reinhardToneMap, acesToneMap, filmicToneMap, gammaCorrection, getToneMapFn,
    decodeRgbe,
    renderHdrToCanvas,
    readCanvasPixelsLinear, readGainMapPixels, applyGainMap,
  };
}
