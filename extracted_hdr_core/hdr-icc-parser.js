/**
 * ============================================================
 *  ICC Profile 解析器 — HDR 色彩空间检测
 *  无外部依赖，纯 JavaScript (需要 Node.js Buffer / zlib)
 * ============================================================
 */

// ============================================================
//  HDR 色彩空间关键词
// ============================================================
const HDR_COLOR_SPACES = [
  'bt2020', 'bt.2020', 'rec2020', 'rec.2020',
  'display p3', 'dci-p3', 'dcip3',
  'scrgb',
  'st.2084', 'st 2084', 'pq',
  'hlg',
  'smpte',
  'hdr',
  'wide gamut', 'widegamut',
];

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
//  ICC Profile 解析
// ============================================================

/**
 * 解析 ICC Profile 数据，检测是否为 HDR 色彩空间
 *
 * @param {Buffer|Uint8Array} iccData - ICC Profile 原始数据
 * @returns {{ isHdr: boolean, colorSpace: string, description: string, profileClass: string, pcs: string } | null}
 */
function parseIccProfile(iccData) {
  try {
    if (!iccData || iccData.length < 128) return null;
    const view = new DataView(iccData.buffer, iccData.byteOffset, iccData.byteLength);

    const profileClass = readStr(view, 12, 4);
    const colorSpace = readStr(view, 16, 4);
    const pcs = readStr(view, 20, 4);

    const tagCount = view.getUint32(128);
    let description = '';

    for (let i = 0; i < tagCount; i++) {
      const tagOffset = 132 + i * 12;
      if (tagOffset + 12 > iccData.length) break;

      const tagSig = readStr(view, tagOffset, 4);
      const tagDataOffset = view.getUint32(tagOffset + 4);
      if (tagDataOffset >= iccData.length) continue;

      // desc 标签: 包含设备描述 (ASCII 或 mluc)
      if (tagSig === 'desc' && tagDataOffset + 4 <= iccData.length) {
        const first4 = readStr(view, tagDataOffset, 4);
        if (first4 === 'mluc' || first4 === 'MLUC') {
          // 多语言 Unicode 格式
          const numRecords = view.getUint32(tagDataOffset + 8);
          if (numRecords > 0 && numRecords < 100) {
            const strLen = view.getUint32(tagDataOffset + 20);
            const strOff = view.getUint32(tagDataOffset + 24);
            if (strLen > 0 && strLen < 512 &&
                tagDataOffset + strOff + strLen <= iccData.length) {
              const bytes = [];
              for (let j = 0; j < strLen; j += 2) {
                const hi = view.getUint8(tagDataOffset + strOff + j);
                const lo = view.getUint8(tagDataOffset + strOff + j + 1);
                const code = (hi << 8) | lo;
                if (code >= 0x20 && code <= 0x7E) bytes.push(code);
                else if (code === 0 && bytes.length > 0) break;
              }
              if (bytes.length > 0) description = String.fromCharCode(...bytes);
            }
          }
        } else {
          // 简单 ASCII
          const descLen = view.getUint32(tagDataOffset);
          if (descLen > 0 && descLen < 256 &&
              tagDataOffset + 4 + descLen <= iccData.length) {
            description = readStr(view, tagDataOffset + 4, Math.min(descLen, 256));
          }
        }
      }
    }

    // 检查描述是否包含 HDR 关键词
    const descLower = description.toLowerCase();
    const isHdr = HDR_COLOR_SPACES.some(keyword => descLower.includes(keyword));

    return {
      isHdr,
      colorSpace: colorSpace.trim(),
      description: description.trim(),
      profileClass,
      pcs,
    };
  } catch (e) {
    console.error('ICC parse error:', e.message);
    return null;
  }
}

// ============================================================
//  JPEG ICC Profile 提取 (从 APP2 ICC_PROFILE 段)
// ============================================================

/**
 * 从 JPEG 文件的 APP2 ICC_PROFILE 段提取 ICC 数据
 *
 * @param {Buffer} fileBuffer - JPEG 文件完整数据
 * @returns {Buffer|null}
 */
function extractJpegIccData(fileBuffer) {
  const view = new DataView(fileBuffer.buffer, fileBuffer.byteOffset, fileBuffer.byteLength);
  let off = 2;
  const chunks = [];

  while (off < view.byteLength - 1) {
    const marker = view.getUint16(off);
    if (marker === 0xFFDA || marker === 0xFFD9) break;
    const segLen = view.getUint16(off + 2);

    if (marker === 0xFFE2) {
      const sig = fileBuffer.toString('ascii', off + 4, off + 15);
      if (sig === 'ICC_PROFILE') {
        const chunkSeq = view.getUint8(off + 16);
        const chunkTotal = view.getUint8(off + 17);
        const iccChunk = fileBuffer.slice(off + 18, off + 2 + segLen);
        chunks[chunkSeq - 1] = iccChunk;

        if (chunks.filter(c => c).length === chunkTotal) {
          return Buffer.concat(chunks);
        }
      }
    }

    off += 2 + segLen;
  }

  if (chunks.length === 1) return chunks[0];
  if (chunks.length > 1) return Buffer.concat(chunks);
  return null;
}

// ============================================================
//  PNG ICC Profile 提取 (从 iCCP 块)
// ============================================================

/**
 * 从 PNG 文件的 iCCP 块提取 ICC Profile 数据
 *
 * @param {Buffer} fileBuffer - PNG 文件完整数据
 * @returns {Buffer|null}
 */
function extractPngIccData(fileBuffer) {
  const pngSig = [137, 80, 78, 71, 13, 10, 26, 10];
  for (let i = 0; i < 8; i++) {
    if (fileBuffer[i] !== pngSig[i]) return null;
  }

  let off = 8;
  while (off + 12 <= fileBuffer.length) {
    const len = fileBuffer.readUInt32BE(off);
    const type = fileBuffer.toString('ascii', off + 4, off + 8);
    if (type === 'IEND') break;

    if (type === 'iCCP') {
      const nameEnd = fileBuffer.indexOf(0, off + 8);
      if (nameEnd > off + 8) {
        const compression = fileBuffer[nameEnd + 1];
        const iccStart = nameEnd + 2;
        const iccLen = len - (iccStart - (off + 8));

        if (iccStart + iccLen <= fileBuffer.length) {
          const iccData = fileBuffer.slice(iccStart, iccStart + iccLen);
          if (compression === 0) {
            // zlib 解压 (Node.js)
            try {
              const zlib = require('zlib');
              return zlib.inflateSync(iccData);
            } catch {
              return iccData;
            }
          }
          return iccData;
        }
      }
    }

    off += 12 + len;
  }
  return null;
}

// ============================================================
//  HDR ICC 检测 (综合入口)
// ============================================================

/**
 * 检测文件是否包含 HDR 内容 (ICC Profile 方式)
 *
 * @param {Buffer} fileBuffer - 文件完整二进制数据
 * @param {string} ext - 文件扩展名 (如 '.jpg', '.png')
 * @returns {{ hasHdrIcc: boolean, iccDescription: string, iccColorSpace: string } | null}
 */
function detectHdrFromIcc(fileBuffer, ext) {
  try {
    let iccData = null;
    let profileName = '';

    if (ext === '.jpg' || ext === '.jpeg') {
      iccData = extractJpegIccData(fileBuffer);
    } else if (ext === '.png') {
      // 同时提取 iCCP 块名称
      const pngSig = [137, 80, 78, 71, 13, 10, 26, 10];
      for (let i = 0; i < 8; i++) if (fileBuffer[i] !== pngSig[i]) return null;
      let off = 8;
      while (off + 12 <= fileBuffer.length) {
        const len = fileBuffer.readUInt32BE(off);
        const type = fileBuffer.toString('ascii', off + 4, off + 8);
        if (type === 'IEND') break;
        if (type === 'iCCP') {
          const nameEnd = fileBuffer.indexOf(0, off + 8);
          if (nameEnd > off + 8) {
            profileName = fileBuffer.toString('utf8', off + 8, nameEnd);
          }
        }
        off += 12 + len;
      }
      iccData = extractPngIccData(fileBuffer);
    }

    if (!iccData) return null;

    const parsed = parseIccProfile(iccData);

    // 合并检查: ICC 描述 + iCCP 名称
    const allText = ((parsed ? parsed.description : '') + ' ' + profileName).toLowerCase();
    const isHdr = HDR_COLOR_SPACES.some(keyword => allText.includes(keyword));

    if (isHdr) {
      return {
        hasHdrIcc: true,
        iccDescription: parsed ? parsed.description : profileName,
        iccColorSpace: parsed ? parsed.colorSpace : 'RGB',
      };
    }

    return parsed
      ? { hasHdrIcc: false, iccDescription: parsed.description, iccColorSpace: parsed.colorSpace }
      : null;
  } catch (e) {
    console.error('ICC 检测失败:', e.message);
  }
  return null;
}

// ============================================================
//  导出
// ============================================================
if (typeof module !== 'undefined' && module.exports) {
  module.exports = {
    HDR_COLOR_SPACES,
    parseIccProfile,
    extractJpegIccData,
    extractPngIccData,
    detectHdrFromIcc,
  };
}
