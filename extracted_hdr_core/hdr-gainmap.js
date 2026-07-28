/**
 * ============================================================
 *  Gain Map (Ultra HDR) 解析器
 *  从 JPEG / PNG 文件中提取 Gain Map 数据
 *
 *  需要 Node.js Buffer (用于文件 I/O)
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
 * 从 JPEG 文件中提取 Gain Map 数据
 *
 * @param {Buffer} fileBuffer - JPEG 文件完整二进制数据
 * @returns {{ gainMapBase64: string, backlight: number, hasGainMap: boolean, iccProfile?: string } | null}
 */
function extractJpegGainMap(fileBuffer) {
  const view = new DataView(fileBuffer.buffer, fileBuffer.byteOffset, fileBuffer.byteLength);

  if (view.getUint16(0) !== JPEG_SOI) return null;

  let off = 2;
  let tiffStart = 0;
  let le = false;
  let mpfDataStart = 0;
  let iccProfile = null;

  // 遍历 JPEG 段
  while (off < view.byteLength - 1) {
    const marker = view.getUint16(off);
    if (marker === JPEG_SOS) break;
    if (marker === JPEG_EOI) break;

    const segLen = view.getUint16(off + 2);

    if (marker === 0xFFE1) {
      const sig = readStr(view, off + 4, 6);
      if (sig === 'Exif\0\0') {
        tiffStart = off + 10;
        le = (view.getUint16(tiffStart) === 0x4949);
        const mpfPtr = findMpfPointer(view, tiffStart, le);
        if (mpfPtr > 0) mpfDataStart = mpfPtr;
      }
    } else if (marker === 0xFFE2) {
      const sig = readStr(view, off + 4, 4);
      if (sig === 'MPF\0' && mpfDataStart === 0) mpfDataStart = off + 8;

      const iccSig = readStr(view, off + 4, 12);
      if (iccSig === 'ICC_PROFILE') {
        iccProfile = fileBuffer.slice(off + 16, off + 2 + segLen).toString('base64');
      }
    }

    off += 2 + segLen;
  }

  if (mpfDataStart > 0) {
    const gainInfo = parseMpf(mpfDataStart, view, le, fileBuffer.length);
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

  if (iccProfile) return { hasGainMap: false, iccProfile };
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
