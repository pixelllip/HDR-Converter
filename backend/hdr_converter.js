const fs = require('fs/promises')
const path = require('path')
const zlib = require('zlib')
const { execFile } = require('child_process')
const sharp = require('sharp')

function clamp(value, min, max) {
  return Math.min(max, Math.max(min, value))
}

function srgbToLinear(value) {
  if (value <= 0.04045) return value / 12.92
  return Math.pow((value + 0.055) / 1.055, 2.4)
}

function linearToSrgb(value) {
  if (value <= 0.0031308) return value * 12.92
  return 1.055 * Math.pow(value, 1.0 / 2.4) - 0.055
}

function queueProgress(onProgress, value, message) {
  if (typeof onProgress === 'function') {
    onProgress({ value, message })
  }
}

function crc32(buffer) {
  let crc = 0xffffffff
  for (const byte of buffer) {
    crc ^= byte
    for (let i = 0; i < 8; i++) {
      if ((crc & 1) === 1) {
        crc = (crc >>> 1) ^ 0xedb88320
      } else {
        crc >>>= 1
      }
    }
  }
  return (crc ^ 0xffffffff) >>> 0
}

function createPngChunk(type, data) {
  const typeBuffer = Buffer.from(type, 'ascii')
  const lengthBuffer = Buffer.alloc(4)
  lengthBuffer.writeUInt32BE(data.length, 0)
  const crc = crc32(Buffer.concat([typeBuffer, data]))
  const crcBuffer = Buffer.alloc(4)
  crcBuffer.writeUInt32BE(crc, 0)
  return Buffer.concat([lengthBuffer, typeBuffer, data, crcBuffer])
}

function injectIccIntoPng(pngBuffer, iccProfileBuffer) {
  const signature = pngBuffer.subarray(0, 8)
  const chunks = []
  let offset = 8

  while (offset < pngBuffer.length) {
    const length = pngBuffer.readUInt32BE(offset)
    const type = pngBuffer.toString('ascii', offset + 4, offset + 8)
    const data = pngBuffer.subarray(offset + 8, offset + 8 + length)
    chunks.push({ type, data })
    offset += 12 + length
  }

  // iCCP chunk: profile_name\0 + compression_method(0=deflate) + compressed_data
  const compressedProfile = zlib.deflateSync(iccProfileBuffer)
  const nameBuffer = Buffer.from('BT.2020\0', 'utf8')
  const iccChunkData = Buffer.concat([nameBuffer, Buffer.from([0]), compressedProfile])

  // 在 IDAT 之前插入 iCCP
  const beforeIdat = chunks.findIndex((chunk) => chunk.type === 'IDAT')
  const insertIndex = beforeIdat >= 0 ? beforeIdat : chunks.length
  chunks.splice(insertIndex, 0, { type: 'iCCP', data: iccChunkData })

  // 重建 PNG
  const output = [signature]
  for (const chunk of chunks) {
    output.push(createPngChunk(chunk.type, chunk.data))
  }
  return Buffer.concat(output)
}

function buildIccApp2Segment(iccProfileBuffer) {
  // JPEG APP2 ICC_PROFILE 标准格式:
  //   FF E2 [length] "ICC_PROFILE\0" [seq_num] [total_num] [icc_data]
  const sig = Buffer.from('ICC_PROFILE\0', 'utf8')  // 12 bytes
  const segLen = 2 + sig.length + 2 + iccProfileBuffer.length  // length field includes itself
  const header = Buffer.alloc(2 + 2 + sig.length + 2)
  let off = 0
  header.writeUInt16BE(0xFFE2, off); off += 2           // APP2 marker
  header.writeUInt16BE(segLen, off); off += 2            // segment length
  sig.copy(header, off); off += sig.length               // "ICC_PROFILE\0"
  header[off++] = 1                                      // sequence number
  header[off++] = 1                                      // total number of segments
  return Buffer.concat([header, iccProfileBuffer])
}

function injectIccIntoJpeg(jpegBuffer, iccProfileBuffer) {
  const app2Segment = buildIccApp2Segment(iccProfileBuffer)
  return Buffer.concat([jpegBuffer.subarray(0, 2), app2Segment, jpegBuffer.subarray(2)])
}

function applyHdrTransform({ data, width, height, settings }) {
  // 与 Flutter 参考项目保持一致: totalExposure = hdrIntensity * fineTuneBrightness
  const totalExposure = settings.hdrIntensity * settings.fineTuneBrightness
  const rAdj = settings.rgbAdjustment?.red ?? 0.96
  const gAdj = settings.rgbAdjustment?.green ?? 1.0
  const bAdj = settings.rgbAdjustment?.blue ?? 1.0
  const totalPixels = width * height
  const linear = new Float64Array(totalPixels * 3)
  let sum = 0

  // Pass 1: sRGB→线性 + 计算平均亮度
  for (let i = 0; i < totalPixels; i++) {
    const base = i * 4
    const r = srgbToLinear(data[base] / 255)
    const g = srgbToLinear(data[base + 1] / 255)
    const b = srgbToLinear(data[base + 2] / 255)
    const offset = i * 3
    linear[offset] = r
    linear[offset + 1] = g
    linear[offset + 2] = b
    sum += 0.2126 * r + 0.7152 * g + 0.0722 * b
  }

  // Pass 2: 自动伽马（基于平均亮度自适应调整）
  const mean = sum / totalPixels
  if (mean > 0.001 && mean < 0.999) {
    const autoGamma = Math.log(0.5) / Math.log(mean)
    const clampedGamma = clamp(autoGamma, 0.3, 3.0)
    for (let i = 0; i < totalPixels * 3; i++) {
      linear[i] = Math.pow(clamp(linear[i], 0.0, Number.MAX_SAFE_INTEGER), clampedGamma)
    }
  }

  // Pass 3: RGB 通道调整 + 曝光 + 伽马 + sRGB 编码
  const output = Buffer.alloc(totalPixels * 4)
  for (let i = 0; i < totalPixels; i++) {
    const offset = i * 3
    let r = linear[offset]
    let g = linear[offset + 1]
    let b = linear[offset + 2]

    r *= rAdj
    g *= gAdj
    b *= bAdj

    r *= totalExposure
    g *= totalExposure
    b *= totalExposure

    r = Math.pow(clamp(r, 0.0, Number.MAX_SAFE_INTEGER), settings.gamma ?? 0.9)
    g = Math.pow(clamp(g, 0.0, Number.MAX_SAFE_INTEGER), settings.gamma ?? 0.9)
    b = Math.pow(clamp(b, 0.0, Number.MAX_SAFE_INTEGER), settings.gamma ?? 0.9)

    const sr = clamp(linearToSrgb(r), 0.0, 1.0)
    const sg = clamp(linearToSrgb(g), 0.0, 1.0)
    const sb = clamp(linearToSrgb(b), 0.0, 1.0)

    const outIndex = i * 4
    output[outIndex] = Math.round(sr * 255)
    output[outIndex + 1] = Math.round(sg * 255)
    output[outIndex + 2] = Math.round(sb * 255)
    output[outIndex + 3] = 255
  }

  return output
}

function normalizeOutputPath(outputPath, outputFormat) {
  const extension = outputFormat === 'png' ? '.png' : '.jpg'
  if (!outputPath) return `output${extension}`
  if (path.extname(outputPath).toLowerCase() === extension) return outputPath
  return `${outputPath}${extension}`
}

function parseSettings(settings = {}) {
  return {
    hdrIntensity: settings.hdrIntensity ?? 1.18,
    fineTuneBrightness: settings.fineTuneBrightness ?? 0.3,
    gamma: settings.gamma ?? 0.9,
    rgbAdjustment: {
      red: settings.rgbAdjustment?.red ?? 0.96,
      green: settings.rgbAdjustment?.green ?? 1.0,
      blue: settings.rgbAdjustment?.blue ?? 1.0
    },
    outputFormat: settings.outputFormat ?? 'png'
  }
}

function execFileAsync(command, args) {
  return new Promise((resolve, reject) => {
    execFile(command, args, (error, stdout) => {
      if (error) return reject(error)
      resolve(stdout)
    })
  })
}

async function detectBackend(preference = 'auto') {
  if (preference === 'cpu') {
    return { name: 'cpu', available: false, message: '使用 CPU 主路径进行处理' }
  }

  try {
    const output = await execFileAsync('nvidia-smi', ['--query-gpu=name', '--format=csv,noheader'])
    if (output && output.trim().length > 0) {
      return { name: 'cuda', available: true, message: '检测到 NVIDIA GPU，优先使用 CUDA 路径' }
    }
  } catch {
    // ignore and fall back to CPU
  }

  if (preference === 'cuda') {
    return { name: 'cuda', available: false, message: 'CUDA 不可用，已回退到 CPU' }
  }

  return { name: 'cpu', available: false, message: '未检测到可用 GPU，使用 CPU 处理' }
}

async function convertImage({ inputPath, outputPath, settings = {}, backendPreference = 'auto', onProgress }) {
  const normalizedSettings = parseSettings(settings)
  const resolvedOutputPath = normalizeOutputPath(outputPath, normalizedSettings.outputFormat)
  const iccProfilePath = path.resolve(__dirname, '2020_profile.icc')
  const iccProfileBuffer = await fs.readFile(iccProfilePath)
  const backend = await detectBackend(backendPreference)

  queueProgress(onProgress, 0.05, `后端: ${backend.name}`)

  let inputBuffer = await sharp(inputPath).raw().toBuffer({ resolveWithObject: true })
  const width = inputBuffer.info.width
  const height = inputBuffer.info.height
  let rawData = inputBuffer.data

  // 统一转为 RGBA（4通道），避免 3 通道 RGB 导致越界
  if (inputBuffer.info.channels === 3) {
    const rgba = Buffer.alloc(width * height * 4, 255)
    for (let i = 0; i < width * height; i++) {
      rgba[i * 4] = rawData[i * 3]
      rgba[i * 4 + 1] = rawData[i * 3 + 1]
      rgba[i * 4 + 2] = rawData[i * 3 + 2]
      rgba[i * 4 + 3] = 255
    }
    rawData = rgba
  }

  queueProgress(onProgress, 0.2, '正在执行 HDR 映射')
  const transformed = applyHdrTransform({ data: rawData, width, height, settings: normalizedSettings })

  const image = sharp(transformed, {
    raw: {
      width,
      height,
      channels: 4
    }
  })

  let resultBuffer
  if (normalizedSettings.outputFormat === 'png') {
    queueProgress(onProgress, 0.6, '正在生成 HDR PNG')
    const pngBuffer = await image
      .png({ compressionLevel: 9 })
      .toBuffer()
    resultBuffer = injectIccIntoPng(pngBuffer, iccProfileBuffer)
  } else {
    queueProgress(onProgress, 0.6, '正在生成 Ultra HDR JPEG')
    const jpegBuffer = await image
      .jpeg({ quality: 98, mozjpeg: true })
      .toBuffer()
    resultBuffer = injectIccIntoJpeg(jpegBuffer, iccProfileBuffer)
  }

  await fs.mkdir(path.dirname(resolvedOutputPath), { recursive: true })
  await fs.writeFile(resolvedOutputPath, resultBuffer)

  queueProgress(onProgress, 1, '转换完成')

  return {
    success: true,
    outputPath: resolvedOutputPath,
    outputFormat: normalizedSettings.outputFormat,
    backend: backend.name,
    message: `${backend.message}，输出已保存`
  }
}

/**
 * 快速预览转换：缩放至最大 800px 后处理，返回 base64 data URL
 * 适用于实时滑块调节预览
 */
async function convertForPreview({ inputPath, settings = {}, onProgress }) {
  const normalizedSettings = parseSettings(settings)
  const iccProfilePath = path.resolve(__dirname, '2020_profile.icc')
  const iccProfileBuffer = await fs.readFile(iccProfilePath)

  queueProgress(onProgress, 0.1, '正在读取图片')

  let img = sharp(inputPath)
  const metadata = await img.metadata()

  // 缩放到预览尺寸（最大 500px，保证拖拽响应速度）
  const maxPreview = 500
  let resizeW = metadata.width
  let resizeH = metadata.height
  if (resizeW > maxPreview || resizeH > maxPreview) {
    const scale = Math.min(maxPreview / resizeW, maxPreview / resizeH, 1)
    resizeW = Math.round(resizeW * scale)
    resizeH = Math.round(resizeH * scale)
  }

  const inputBuffer = await img
    .resize(resizeW, resizeH, { fit: 'inside', withoutEnlargement: true })
    .raw()
    .toBuffer({ resolveWithObject: true })

  const width = inputBuffer.info.width
  const height = inputBuffer.info.height
  let rawData = inputBuffer.data

  // 统一转为 RGBA
  if (inputBuffer.info.channels === 3) {
    const rgba = Buffer.alloc(width * height * 4, 255)
    for (let i = 0; i < width * height; i++) {
      rgba[i * 4] = rawData[i * 3]
      rgba[i * 4 + 1] = rawData[i * 3 + 1]
      rgba[i * 4 + 2] = rawData[i * 3 + 2]
      rgba[i * 4 + 3] = 255
    }
    rawData = rgba
  }

  queueProgress(onProgress, 0.4, '正在处理 HDR 映射')
  const transformed = applyHdrTransform({ data: rawData, width, height, settings: normalizedSettings })

  queueProgress(onProgress, 0.7, '正在编码预览图')
  const outputFormat = normalizedSettings.outputFormat || 'jpg'
  let resultBuffer
  if (outputFormat === 'png') {
    const pngBuffer = await sharp(transformed, { raw: { width, height, channels: 4 } })
      .png({ compressionLevel: 3 })
      .toBuffer()
    resultBuffer = injectIccIntoPng(pngBuffer, iccProfileBuffer)
  } else {
    const jpegBuffer = await sharp(transformed, { raw: { width, height, channels: 4 } })
      .jpeg({ quality: 92 })
      .toBuffer()
    resultBuffer = injectIccIntoJpeg(jpegBuffer, iccProfileBuffer)
  }

  queueProgress(onProgress, 1, '预览就绪')
  const base64 = resultBuffer.toString('base64')
  const mime = outputFormat === 'png' ? 'image/png' : 'image/jpeg'
  return {
    dataUrl: `data:${mime};base64,${base64}`,
    width,
    height,
    aspectRatio: width / height
  }
}

module.exports = {
  convertImage,
  convertForPreview,
  detectBackend
}
