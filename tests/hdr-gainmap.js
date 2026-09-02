/**
 * ============================================================
 *  Gain Map (Ultra HDR) 解析器
 *  从 JPEG / PNG 文件中提取 Gain Map 数据
 *
 *  需要 Node.js Buffer (用于文件 I/O)
 *
 *  注：自 extracted_hdr_core/ 迁入的测试工具（应用运行时不再使用该模块，
 *  增益图逻辑已由 Rust 后端 ultra_hdr.rs 实现；本文件仅供 tests/ 校验用）。
 * ============================================================
 */

// JPEG markers
const JPEG_SOI = 0xFFD8;
const JPEG_SOS = 0xFFDA;
const JPEG_EOI = 0xFFD9;

// Exif tags
const TAG_EXIF_IFD = 0x8769;
const TAG_MPF_PTR = 0x927c;

// MPF tags
const TAG_MPF_NUM_IMAGES = 0xB001;
const TAG_MPF_ENTRY = 0xB002;

// ============================================================
//  工具函数
// ============================================================

function readStr(view, off, len) {
  let s = '';
  for (let i = 0; i < len && off + i < view.byteLength; i++) {
    const c = view.getUint8(off + i);
    if (c === 0) break;
    s += String.fromCharCode(c);
  }
  return s;
}

/** 读取原始字节为字符串（不因 \0 提前截断） */
function readRaw(view, off, len) {
  let s = '';
  for (let i = 0; i < len && off + i < view.byteLength; i++) {
    s += String.fromCharCode(view.getUint8(off + i));
  }
  return s;
}

// ============================================================
//  JPEG SOI/EOI 扫描（与 libultrahdr JpegScanner 一致，不依赖 MPF 偏移）
// ============================================================

/**
 * 从 start 开始查找第一个 JPEG SOI (FF D8)，并返回其 [start, end(含EOI)]
 */
function findNextJpeg(view, start, fileLen) {
  for (let i = start; i < fileLen - 1; i++) {
    if (view.getUint8(i) === 0xFF && view.getUint8(i + 1) === 0xD8) {
      const end = findJpegEoi(view, i + 2, fileLen);
      if (end > 0) return { start: i, end };
      return { start: i, end: fileLen };
    }
  }
  return null;
}

/**
 * 从 start（SOI 之后）查找本 JPEG 的 EOI (FF D9)，返回 EOI 之后的位置
 * 正确处理熵编码数据的字节填充（FF 00）
 */
function findJpegEoi(view, start, fileLen) {
  let p = start;
  while (p < fileLen - 1) {
    if (view.getUint8(p) !== 0xFF) { p++; continue }
    const m = view.getUint8(p + 1);
    if (m === 0x00) { p += 2; continue } // 字节填充
    if (m >= 0xD0 && m <= 0xD7) { p += 2; continue } // RSTn
    if (m === 0xD9) return p + 2; // EOI
    if (m === 0xDA) {
      // SOS: 段头之后是熵编码数据，逐字节找 EOI（处理填充）
      p += 2 + view.getUint16(p + 2);
      for (let q = p; q < fileLen - 1; q++) {
        if (view.getUint8(q) === 0xFF && view.getUint8(q + 1) === 0x00) { q++; continue }
        if (view.getUint8(q) === 0xFF && view.getUint8(q + 1) === 0xD9) return q + 2;
      }
      return -1;
    }
    if (p + 4 > fileLen) return -1;
    p += 2 + view.getUint16(p + 2);
  }
  return -1;
}

/** 提取 JPEG 区域内的第一个 APP1 XMP 文本 */
function extractApp1Xmp(view, start, end) {
  let p = start + 2; // 跳过 SOI
  while (p + 4 <= end) {
    if (view.getUint8(p) !== 0xFF) break;
    const marker = view.getUint16(p);
    if (marker === 0xFFDA || marker === 0xFFD9) break;
    const segLen = view.getUint16(p + 2);
    if (marker === 0xFFE1) {
      const ns = readStr(view, p + 4, 29);
      if (ns === 'http://ns.adobe.com/xap/1.0/') {
        return readStr(view, p + 4 + 29, segLen - 29);
      }
    }
    p += 2 + segLen;
  }
  return null;
}

/** 解析 SOF 段的尺寸（宽/高） */
function parseSofDimensions(view, start, end) {
  let p = start + 2;
  while (p + 4 <= end) {
    if (view.getUint8(p) !== 0xFF) break;
    const marker = view.getUint16(p);
    if (marker === 0xFFDA || marker === 0xFFD9) break;
    if (marker >= 0xFFC0 && marker <= 0xFFCF && marker !== 0xFFC4 && marker !== 0xFFC8 && marker !== 0xFFCC) {
      return {
        width: view.getUint16(p + 7),
        height: view.getUint16(p + 5),
        numComponents: view.getUint8(p + 9),
      };
    }
    p += 2 + view.getUint16(p + 2);
  }
  return null;
}

// ============================================================
//  hdrgm XMP 元数据解析
// ============================================================

/**
 * 从增益图 XMP 解析 hdrgm 元数据（Android Ultra HDR 规范）
 * @returns {{ version, gainMapMinLog2, gainMapMaxLog2, gamma, offsetSdr, offsetHdr,
 *             hdrCapacityMinLog2, hdrCapacityMaxLog2, baseRenditionIsHdr } | null}
 */
function parseXmpGainMapMetadata(xmp) {
  if (!xmp) return null;
  const attr = (name) => {
    const m = xmp.match(new RegExp(name + '="([^"]+)"'));
    return m ? parseFloat(m[1]) : null;
  };
  const version = attr('hdrgm:Version');
  if (version !== 1.0) return null; // 必须是 1.0
  const gainMapMaxLog2 = attr('hdrgm:GainMapMax');
  if (gainMapMaxLog2 === null || !isFinite(gainMapMaxLog2)) return null;
  return {
    version: '1.0',
    gainMapMinLog2: attr('hdrgm:GainMapMin') ?? 0,
    gainMapMaxLog2,
    gamma: attr('hdrgm:Gamma') ?? 1,
    offsetSdr: attr('hdrgm:OffsetSDR') ?? 1 / 64,
    offsetHdr: attr('hdrgm:OffsetHDR') ?? 1 / 64,
    hdrCapacityMinLog2: attr('hdrgm:HDRCapacityMin') ?? 0,
    hdrCapacityMaxLog2: attr('hdrgm:HDRCapacityMax') ?? gainMapMaxLog2,
    baseRenditionIsHdr: (xmp.match(/hdrgm:BaseRenditionIsHDR="([^"]+)"/) || [null, 'False'])[1] === 'True',
  };
}

// ============================================================
//  MPF (Multi-Picture Format) 解析
// ============================================================

/**
 * 从 Exif 子 IFD 中找到 MPF 指针 (TAG 0x927c)
 *
 * @param {DataView} view - 完整文件 DataView
 * @param {number} tiffStart - TIFF 头在文件中的绝对偏移
 * @param {boolean} le - 是否小端字节序
 * @returns {number} MPF 数据的绝对偏移，0 表示未找到
 */
function findMpfPointer(view, tiffStart, le) {
  const ifd0Off = view.getUint32(tiffStart + 4, le);
  if (ifd0Off === 0) return 0;
  const ifd0Abs = tiffStart + ifd0Off;
  const n0 = view.getUint16(ifd0Abs, le);

  for (let i = 0; i < n0; i++) {
    const eo = ifd0Abs + 2 + i * 12;
    if (eo + 12 > view.byteLength) break;
    if (view.getUint16(eo, le) === TAG_EXIF_IFD) {
      const exifIfdOff = view.getUint32(eo + 8, le);
      if (exifIfdOff === 0) continue;
      const exifAbs = tiffStart + exifIfdOff;
      const nExif = view.getUint16(exifAbs, le);
      for (let j = 0; j < nExif; j++) {
        const exeo = exifAbs + 2 + j * 12;
        if (exeo + 12 > view.byteLength) break;
        if (view.getUint16(exeo, le) === TAG_MPF_PTR) {
          return tiffStart + view.getUint32(exeo + 8, le);
        }
      }
    }
  }
  return 0;
}

/**
 * 解析 MPF IFD，提取 Gain Map 图像信息
 *
 * @param {number} mpfDataStart - MPF TIFF 头在文件中的绝对偏移
 * @param {DataView} view - 完整文件 DataView
 * @param {boolean} le - Exif 字节序
 * @param {number} fileLen - 文件总长度
 * @returns {{ offset: number, length: number, backlight: number, width: number, height: number } | null}
 */
function parseMpf(mpfDataStart, view, le, fileLen) {
  const mpfLE = (view.getUint16(mpfDataStart) === 0x4949);
  const mpfMagic = view.getUint16(mpfDataStart + 2, mpfLE);

  if (mpfMagic !== 0x002A) return null;

  const mpfIfdOff = view.getUint32(mpfDataStart + 4, mpfLE);
  const mpfIfdAbs = mpfDataStart + mpfIfdOff;
  const n = view.getUint16(mpfIfdAbs, mpfLE);

  let numImages = 0;
  let entryDataAbs = 0;

  for (let i = 0; i < n; i++) {
    const eo = mpfIfdAbs + 2 + i * 12;
    if (eo + 12 > view.byteLength) break;
    const tag = view.getUint16(eo, mpfLE);
    const cnt = view.getUint32(eo + 4, mpfLE);
    const val = view.getUint32(eo + 8, mpfLE);

    if (tag === TAG_MPF_NUM_IMAGES) {
      numImages = cnt;
    } else if (tag === TAG_MPF_ENTRY) {
      entryDataAbs = mpfIfdAbs + val;
    }
  }

  if (numImages < 2 || entryDataAbs === 0) return null;

  // Entry 0 = 主图像, Entry 1 = Gain Map
  const entrySize = 16;
  const e1Abs = entryDataAbs + entrySize;

  const attr1 = view.getUint32(e1Abs, mpfLE);
  const rawOff1 = view.getUint32(e1Abs + 4, mpfLE);
  const size1 = view.getUint32(e1Abs + 8, mpfLE);

  // 计算 Gain Map 实际偏移
  const off0 = view.getUint32(entryDataAbs + 4, mpfLE);
  const size0 = view.getUint32(entryDataAbs + 8, mpfLE);

  let gainMapFileOff = rawOff1;
  if (rawOff1 < off0 + size0 && rawOff1 > 0) {
    gainMapFileOff = off0 + size0 + rawOff1;
  } else if (rawOff1 === 0) {
    gainMapFileOff = off0 + size0;
  }

  if (gainMapFileOff > 0 && size1 > 0 && gainMapFileOff + size1 <= fileLen) {
    const backlight = ((attr1 >> 8) & 0xFF) / 16;
    return {
      offset: gainMapFileOff,
      length: size1,
      backlight: Math.max(1, backlight) || 4.0,
      width: (attr1 >> 16) & 0xFFFF,
      height: attr1 & 0xFFFF,
    };
  }

  return null;
}

// ============================================================
//  JPEG Gain Map 提取 (通过 MPF)
// ============================================================

/**
 * 从 JPEG 文件中提取 Gain Map 数据（Android Ultra HDR 格式）
 *
 * @param {Buffer} fileBuffer - JPEG 文件完整二进制数据
 * @returns {{ gainMapBase64: string, backlight: number, metadata?: object, hasGainMap: boolean, iccProfile?: string, primaryXmp?: string } | null}
 */
function extractJpegGainMap(fileBuffer) {
  const view = new DataView(fileBuffer.buffer, fileBuffer.byteOffset, fileBuffer.byteLength);

  if (view.getUint16(0) !== JPEG_SOI) return null;

  let primaryXmp = null;
  let iccProfile = null;

  // 1) 扫描主图像段（到 SOS 为止），收集 XMP / ICC
  let off = 2;
  while (off < view.byteLength - 1) {
    const marker = view.getUint16(off);
    if (marker === JPEG_SOS || marker === JPEG_EOI) break;
    if (off + 4 > view.byteLength) break;
    const segLen = view.getUint16(off + 2);
    if (marker === 0xFFE1) {
      const ns = readStr(view, off + 4, 29);
      if (ns === 'http://ns.adobe.com/xap/1.0/') {
        primaryXmp = readStr(view, off + 4 + 29, segLen - 29);
      }
    } else if (marker === 0xFFE2) {
      const iccSig = readRaw(view, off + 4, 12);
      if (iccSig === 'ICC_PROFILE\0') {
        iccProfile = fileBuffer.slice(off + 16, off + 2 + segLen).toString('base64');
      }
    }
    off += 2 + segLen;
  }

  // 2) 定位次图像（增益图）：主图像 EOI 之后的下一个 JPEG（与 libultrahdr 一致）
  const primaryEoi = findJpegEoi(view, 2, view.byteLength);
  const secondary = primaryEoi > 0 ? findNextJpeg(view, primaryEoi, view.byteLength) : null;

  if (secondary) {
    const gmData = fileBuffer.slice(secondary.start, secondary.end);
    const gmXmp = extractApp1Xmp(view, secondary.start, secondary.end);
    const metadata = parseXmpGainMapMetadata(gmXmp);
    const dims = parseSofDimensions(view, secondary.start, secondary.end);

    // 回退: 无 hdrgm 元数据时用 MPF 里的 backlight
    let backlight = 4.0;
    if (metadata && metadata.gainMapMaxLog2 !== null && isFinite(metadata.gainMapMaxLog2)) {
      backlight = Math.pow(2, metadata.gainMapMaxLog2);
    }
    if (metadata) {
      // 优先用 HDRCapacityMax（显示器能力上限）
      if (metadata.hdrCapacityMaxLog2 !== null && isFinite(metadata.hdrCapacityMaxLog2)) {
        backlight = Math.pow(2, metadata.hdrCapacityMaxLog2);
      }
    }

    return {
      gainMapBase64: gmData.toString('base64'),
      gainMapWidth: dims ? dims.width : 0,
      gainMapHeight: dims ? dims.height : 0,
      numComponents: dims ? dims.numComponents : 0,
      backlight: Math.max(1, backlight),
      metadata,
      hasGainMap: true,
      iccProfile,
      primaryXmp,
    };
  }

  if (iccProfile) return { hasGainMap: false, iccProfile, primaryXmp };
  return null;
}

// ============================================================
//  PNG Gain Map 提取
// ============================================================

/**
 * 从 PNG 文件中提取 Gain Map 数据
 *
 * @param {Buffer} fileBuffer - PNG 文件完整二进制数据
 * @returns {{ gainMapBase64: string, backlight: number, hasGainMap: boolean, iccProfile?: string } | null}
 */
function extractPngGainMap(fileBuffer) {
  const view = new DataView(fileBuffer.buffer, fileBuffer.byteOffset, fileBuffer.byteLength);
  const pngSig = [137, 80, 78, 71, 13, 10, 26, 10];
  for (let i = 0; i < 8 && i < fileBuffer.length; i++) {
    if (view.getUint8(i) !== pngSig[i]) return null;
  }

  let offset = 8;
  let iccProfile = null;
  let exifDataOffset = -1;
  let exifDataLen = 0;

  while (offset + 12 <= fileBuffer.length) {
    const length = view.getUint32(offset);
    const chunkType = readStr(view, offset + 4, 4);
    if (chunkType === 'IEND') break;

    if (chunkType === 'eXIf') {
      exifDataOffset = offset + 8;
      exifDataLen = length;
    }

    if (chunkType === 'iCCP') {
      const nameEnd = fileBuffer.indexOf(0, offset + 8);
      if (nameEnd > 0) {
        iccProfile = fileBuffer.slice(offset + 8, offset + 8 + length).toString('base64');
      }
    }

    // 自定义 Gain Map 块
    if (chunkType === 'gaMa' || chunkType === 'hdrG') {
      if (length > 4) {
        return {
          hasGainMap: true,
          gainMapBase64: fileBuffer.slice(offset + 8, offset + 8 + length).toString('base64'),
          gainMapWidth: 0,
          gainMapHeight: 0,
          backlight: 4.0,
          iccProfile,
        };
      }
    }

    offset += 12 + length;
  }

  // 尝试从 eXIf 块解析 MPF
  if (exifDataOffset > 0 && exifDataLen > 0) {
    try {
      const exifStart = exifDataOffset;
      const le = view.getUint16(exifStart) === 0x4949;
      const mpfPtr = findMpfPointer(view, exifStart, le);

      if (mpfPtr > 0) {
        const gainInfo = parseMpf(mpfPtr, view, le, fileBuffer.length);
        if (gainInfo && gainInfo.offset > 0 && gainInfo.length > 0) {
          const end = gainInfo.offset + gainInfo.length;
          if (end <= fileBuffer.length) {
            const gmData = fileBuffer.slice(gainInfo.offset, end);
            return {
              gainMapBase64: gmData.toString('base64'),
              gainMapWidth: gainInfo.width || 0,
              gainMapHeight: gainInfo.height || 0,
              backlight: gainInfo.backlight || 4.0,
              hasGainMap: true,
              iccProfile,
            };
          }
        }
      }
    } catch (e) {
      console.error('PNG eXIf/MPF 解析失败:', e.message);
    }
  }

  if (iccProfile) return { hasGainMap: false, iccProfile };
  return null;
}

// ============================================================
//  Gain Map 检测入口
// ============================================================

/**
 * 检测文件是否包含 Gain Map，并提取增益图数据
 *
 * @param {Buffer} fileBuffer - 文件完整二进制数据
 * @param {string} ext - 文件扩展名 (如 '.jpg', '.png')
 * @returns {{ gainMapBase64: string, backlight: number, hasGainMap: boolean, iccProfile?: string } | null}
 */
function detectAndExtractGainMap(fileBuffer, ext) {
  try {
    if (ext === '.jpg' || ext === '.jpeg') {
      return extractJpegGainMap(fileBuffer);
    } else if (ext === '.png') {
      return extractPngGainMap(fileBuffer);
    }
  } catch (e) {
    console.error('Gain Map 解析失败:', e.message);
  }
  return null;
}

// ============================================================
//  导出
// ============================================================
if (typeof module !== 'undefined' && module.exports) {
  module.exports = {
    findMpfPointer,
    parseMpf,
    extractJpegGainMap,
    extractPngGainMap,
    detectAndExtractGainMap,
  };
}
