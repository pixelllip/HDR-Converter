'use strict'
/**
 * hevc_inject.js — 按 AUD（nal_unit_type=35）为 AN ACCESS UNIT 注入 Prefix_SEI NAL
 *
 * 输入/输出均为 Annex B 的 HEVC 裸流。对每个 AUD：在其后、VCL 前插入一个
 * Prefix_SEI（user_data_registered_itu_t_t35），载荷由调用方提供（每个 AU 可不同，
 * 用于验证“动态元数据逐窗”）。
 */
const fs = require('fs')
const { buildPrefixSeiNal } = require('./st2094_50')

const START = 1 // 3 字节开始码 000001；4 字节 00000001 也兼容（在解析时统一处理）

/** 把 Annex B 流切分为 NAL 数组：{ start, len, type, raw }，raw 含开始码 */
function splitNalUnits(buf) {
  const nals = []
  let i = 0
  const starts = []
  while (i < buf.length - 3) {
    if (buf[i] === 0 && buf[i + 1] === 0) {
      if (buf[i + 2] === 1) { starts.push(i); i += 3; continue }
      if (i + 3 < buf.length && buf[i + 2] === 0 && buf[i + 3] === 1) { starts.push(i); i += 4; continue }
    }
    i++
  }
  for (let k = 0; k < starts.length; k++) {
    const off = starts[k]
    const end = (k + 1 < starts.length) ? starts[k + 1] : buf.length
    nals.push({ start: off, end, raw: buf.subarray(off, end) })
  }
  return nals
}

function nalType(nals, idx) {
  // 跳过开始码，取第一个字节
  const r = nals[idx].raw
  let p = 0
  if (r[p] === 0 && r[p + 1] === 0) p = (r[p + 2] === 1) ? 3 : 4
  return (r[p] >> 1) & 0x3F
}

/** 在每帧 AUD 后注入 Prefix_SEI。
 *  @param srcBuf HEVC Annex B
 *  @param payloadForAu (auIndex, totalAUs) => Buffer（T.35 载荷）
 *  @param buildSeiNal 由 st2094_50 提供（默认 Prefix）
 */
function injectSeiPerAu(srcBuf, payloadForAu, { buildSeiNal, position = 'after-aud' } = {}) {
  const build = buildSeiNal || buildPrefixSeiNal // 默认 Prefix_SEI
  const nals = splitNalUnits(srcBuf)
  const audIdx = []
  for (let i = 0; i < nals.length; i++) if (nalType(nals, i) === 35) audIdx.push(i)
  if (audIdx.length === 0) throw new Error('未找到 AUD（建议 x265 加 -x265-params aud=1 重新生成）')

  const out = []
  let au = 0
  for (let i = 0; i < nals.length; i++) {
    const isAud = nalType(nals, i) === 35
    // 后缀插入：在“下一个 AUD 之前”即当前 AU 末尾（第一个 AU 无前导位置，可忽略）
    if (position === 'before-aud' && isAud && audIdx.indexOf(i) > 0) {
      const k = audIdx.indexOf(i) - 1
      out.push(Buffer.concat([Buffer.from([0, 0, 0, 1]), build(payloadForAu(k, audIdx.length))]))
    }
    out.push(nals[i].raw)
    if (position === 'after-aud' && isAud) {
      const payload = payloadForAu(au, audIdx.length)
      out.push(Buffer.concat([Buffer.from([0, 0, 0, 1]), build(payload)]))
      au++
    }
  }
  return Buffer.concat(out)
}

/** 简单封装：整段同载荷 */
function injectSei(srcPath, dstPath, t35Buf, { buildSeiNal } = {}) {
  const src = fs.readFileSync(srcPath)
  const out = injectSeiPerAu(src, () => t35Buf, { buildSeiNal })
  fs.writeFileSync(dstPath, out)
  return { inNals: splitNalUnits(src).length, outBytes: out.length }
}

module.exports = { splitNalUnits, nalType, injectSeiPerAu, injectSei }
