'use strict'
/**
 * st2094_50.js — SMPTE ST 2094-50 (Application #5) 最小实现
 *
 * P0 最小闭环验证模块：
 *  1) Annex C 二进制编码/解码（规范 C.2 语法 / C.3 语义的“原始 u16 层”）；
 *  2) 组装 ITU-T T.35 载荷（country 0xB5 / provider 0x0090 / oriented 0x0001 → 即 B5 00 90 00 01）；
 *  3) 组装 HEVC Prefix/Suffix_SEI NAL（user_data_registered_itu_t_t35，payload_type=4），含 EBSP 转义。
 *
 * 说明：本实现面向“最小可验证”，数值按草案语义保留为 raw u16（declared 1/1000、1/10000 等缩放
 * 由上层负责换算）；参考白定点缩放（÷15 语义）在草案 HTML 中疑似排版错乱，见 02 文档风险 #2。
 */

const T35_COUNTRY_US = 0xB5
const T35_PROVIDER_SMPTE = 0x0090
const T35_ORIENTED_APP5 = 0x0001

// ---------- 底层字节工具 ----------
function u16(v) {
  const b = Buffer.alloc(2)
  b.writeUInt16BE(v >>> 0 & 0xFFFF, 0)
  return b
}
function concat(...arrs) {
  const bufs = arrs.flat().map(a => a === undefined ? [] : (a.length === undefined ? [a] : a))
  return Buffer.concat(bufs.filter(Buffer.isBuffer))
}
function hex(b) { return b.toString('hex').toUpperCase() }

// ---------- Annex C 编码（C.2 语法） ----------

/** 编码 smpte_st_2094_50_color_volume_transform()
 *  @param cvt { hasCustomHdrReferenceWhite, hdrReferenceWhite?, hasAdaptiveToneMap, adaptiveToneMap? }
 */
function encodeColorVolumeTransform(cvt) {
  const flagByte = (cvt.hasCustomHdrReferenceWhite ? 0x80 : 0) | (cvt.hasAdaptiveToneMap ? 0x40 : 0)
  const parts = [Buffer.from([flagByte])]
  if (cvt.hasCustomHdrReferenceWhite) parts.push(u16(cvt.hdrReferenceWhite))
  if (cvt.hasAdaptiveToneMap) parts.push(encodeAdaptiveToneMap(cvt.adaptiveToneMap))
  return Buffer.concat(parts)
}

/** 编码 smpte_st_2094_50_adaptive_tone_map()
 *  @param atm {
 *    baselineHdrHeadroom,                      // raw u16（×10000 缩放给上层）
 *    useReferenceWhiteToneMapping: bool,       // 1 → 走 C.3.8 配方（其余字段省略）
 *    numAlternateImages?, chromaticitiesMode?, hasCommonComponentMixParams?, hasCommonCurveParams?,
 *    chromaticities?[8],                       // raw u16（仅 mode==3 时出现）
 *    alternates?: [{ headroom, mixing, curve }]
 *  }
 */
function encodeAdaptiveToneMap(atm) {
  if (atm.useReferenceWhiteToneMapping) {
    // baseline u16 + 1bit 标志 + 7 位 reserved 0 → 0x80
    return Buffer.concat([u16(atm.baselineHdrHeadroom), Buffer.from([0x80])])
  }
  // 显式分支（flag=0 在 b7）：b6..b4=num_alt，b3..b2=chromaticities_mode，b1=common_mix，b0=common_curve
  const byte = ((atm.numAlternateImages & 0x07) << 4)
    | ((atm.chromaticitiesMode & 0x03) << 2)
    | ((atm.hasCommonComponentMixParams ? 1 : 0) << 1)
    | (atm.hasCommonCurveParams ? 1 : 0)
  const parts = [u16(atm.baselineHdrHeadroom), Buffer.from([byte])]
  if (atm.chromaticitiesMode === 3) {
    for (let r = 0; r < 8; r++) parts.push(u16(atm.chromaticities[r]))
  }
  const nAlt = Math.min(atm.numAlternateImages, 4)
  for (let a = 0; a < nAlt; a++) {
    const alt = atm.alternates[a]
    parts.push(u16(alt.headroom))
    parts.push(encodeComponentMixing(alt.mixing, a, atm.hasCommonComponentMixParams))
    parts.push(encodeGainCurve(alt.curve, a, atm.hasCommonCurveParams))
  }
  return Buffer.concat(parts)
}

/** 编码 smpte_st_2094_50_component_mixing()
 *  @param m { type: 0..3, coeff?: [6] raw u16 }
 */
function encodeComponentMixing(m, a, hasCommon) {
  if (a > 0 && hasCommon) return Buffer.alloc(0) // 值继承自 [0]
  const type = m.type
  if (type !== 3) {
    return Buffer.from([(type << 6) & 0xFF]) // type u2 + reserved 6
  }
  // type==3: type u2 + 6 个 has 标志，然后按标志写系数
  const flags = m.coeff.map(v => (v > 0 ? 1 : 0))
  let flagByte = (type << 6)
  for (let k = 0; k < 6; k++) flagByte |= (flags[k] << (5 - k))
  const parts = [Buffer.from([flagByte])]
  for (let k = 0; k < 6; k++) if (flags[k]) parts.push(u16(m.coeff[k]))
  return Buffer.concat(parts)
}

/** 编码 smpte_st_2094_50_gain_curve()
 *  @param g { ncp, pchip:bool, x:[], y:[], theta:[]? }  // theta 仅 !pchip 时需要
 */
function encodeGainCurve(g, a, hasCommon) {
  const parts = []
  if (!(a > 0 && hasCommon)) {
    const byte = (((g.ncp - 1) & 0x1F) << 3) | ((g.pchip ? 1 : 0) << 2) // 5+1+2
    parts.push(Buffer.from([byte]))
    for (let c = 0; c < g.ncp; c++) parts.push(u16(g.x[c]))
  }
  for (let c = 0; c < g.ncp; c++) parts.push(u16(g.y[c]))
  if (!g.pchip) for (let c = 0; c < g.ncp; c++) parts.push(u16(g.theta[c]))
  return Buffer.concat(parts)
}

/** 编码完整 smpte_st_2094_50_application_info()
 *  @param info { applicationVersion?, minimumApplicationVersion?, colorVolumeTransform }
 */
function encodeApplicationInfo(info) {
  const flagByte = (((info.applicationVersion ?? 0) & 0x07) << 5)
    | (((info.minimumApplicationVersion ?? 0) & 0x07) << 2) // + 2 位 reserved
  return Buffer.concat([Buffer.from([flagByte]), encodeColorVolumeTransform(info.colorVolumeTransform)])
}

// ---------- anneC 解码（C.3 语义，原始层） ----------

class Reader {
  constructor(buf) { this.buf = buf; this.off = 0 }
  byte() { if (this.off >= this.buf.length) throw new Error('truncated'); return this.buf[this.off++] }
  u16() {
    const b0 = this.byte(), b1 = this.byte()
    return (b0 << 8) | b1
  }
}

function decodeColorVolumeTransform(r) {
  const f = r.byte()
  const cvt = { hasCustomHdrReferenceWhite: !!(f & 0x80), hasAdaptiveToneMap: !!(f & 0x40) }
  if (cvt.hasCustomHdrReferenceWhite) cvt.hdrReferenceWhite = r.u16()
  if (cvt.hasAdaptiveToneMap) {
    cvt.adaptiveToneMap = decodeAdaptiveToneMap(r)
  }
  return cvt
}

function decodeAdaptiveToneMap(r) {
  const atm = { baselineHdrHeadroom: r.u16() }
  const f = r.byte()
  atm.useReferenceWhiteToneMapping = !!((f >> 7) & 0x01)
  if (atm.useReferenceWhiteToneMapping) {
    return atm // 其余 7 位 reserved
  }
  atm.numAlternateImages = (f >> 4) & 0x07
  atm.chromaticitiesMode = (f >> 2) & 0x03
  atm.hasCommonComponentMixParams = !!((f >> 1) & 0x01)
  atm.hasCommonCurveParams = !!(f & 0x01)
  if (atm.chromaticitiesMode === 3) {
    atm.chromaticities = []
    for (let k = 0; k < 8; k++) atm.chromaticities.push(r.u16())
  }
  atm.alternates = []
  for (let a = 0; a < Math.min(atm.numAlternateImages, 4); a++) {
    const alt = { headroom: r.u16() }
    // component_mixing 继承规则镜像：a>0 且 common 时无字节
    const mixing = { type: (a > 0 && atm.hasCommonComponentMixParams) ? atm.alternates[0].mixing.type : undefined }
    if (a > 0 && atm.hasCommonComponentMixParams) {
      const m0 = atm.alternates[0].mixing
      mixing.type = m0.type
      if (m0.type === 3) mixing.coeff = m0.coeff.slice()
      alt.mixing = mixing
    } else {
      alt.mixing = decodeComponentMixing(r)
    }
    // gain curve
    const curvePrev = (a > 0 && atm.hasCommonCurveParams) ? atm.alternates[0].curve : null
    alt.curve = decodeGainCurve(r, curvePrev)
    atm.alternates.push(alt)
  }
  return atm
}

function decodeComponentMixing(r) {
  const f = r.byte()
  const type = (f >> 6) & 0x03
  if (type !== 3) return { type }
  const flags = []
  for (let k = 0; k < 6; k++) flags.push((f >> (5 - k)) & 0x01)
  const coeff = []
  for (let k = 0; k < 6; k++) coeff.push(flags[k] ? r.u16() : 0)
  return { type, coeff }
}

function decodeGainCurve(r, curvePrev) {
  let pchip, ncp
  if (curvePrev) {
    // a>0 且 common：ncp/pchip/x 继承 [0]
    pchip = curvePrev.pchip
    ncp = curvePrev.ncp
  } else {
    const f = r.byte()
    ncp = ((f >> 3) & 0x1F) + 1
    pchip = !!((f >> 2) & 0x01)
  }
  const g = { ncp, pchip, x: [], y: [], theta: [] }
  if (curvePrev) {
    g.x = curvePrev.x.slice()
  } else {
    for (let c = 0; c < ncp; c++) g.x.push(r.u16())
  }
  for (let c = 0; c < ncp; c++) g.y.push(r.u16())
  if (!pchip) for (let c = 0; c < ncp; c++) g.theta.push(r.u16())
  return g
}

function decodeApplicationInfo(buf) {
  const r = new Reader(buf)
  const f = r.byte()
  const info = {
    applicationVersion: (f >> 5) & 0x07,
    minimumApplicationVersion: (f >> 2) & 0x07
  }
  info.colorVolumeTransform = decodeColorVolumeTransform(r)
  return { info, bytesRead: r.off }
}

// ---------- T.35 载荷 / HEVC SEI ----------

/** 给 Application #5 元数据加 T.35 前缀 → B5 00 90 00 01 + appInfo */
function t35Payload(appInfoBuf) {
  const h = Buffer.from([
    T35_COUNTRY_US,
    (T35_PROVIDER_SMPTE >> 8) & 0xFF, T35_PROVIDER_SMPTE & 0xFF,
    (T35_ORIENTED_APP5 >> 8) & 0xFF, T35_ORIENTED_APP5 & 0xFF
  ])
  return Buffer.concat([h, appInfoBuf])
}

/** SEI payload_size 的 0xFF 续段编码（≤255 时单字节） */
function encodeSize(v) {
  const out = []
  while (v >= 0xFF) { out.push(0xFF); v -= 0xFF }
  out.push(v)
  return out
}

/** 构造 SEI RBSP：一条 user_data_registered_itu_t_t35（payload_type=4）消息 + rbsp_stop */
function seiRbsp(t35Buf) {
  const parts = [0x04]               // payload_type = 4
  parts.push(...encodeSize(t35Buf.length))
  const msg = Buffer.concat([Buffer.from(parts), t35Buf])
  const rbsp = Buffer.concat([msg, Buffer.from([0x80])]) // rbsp_stop_one_bit（此前字节对齐 → 追加 0x80）
  return rbsp
}

/** EBSP 转义（00 00 00/01/02/03 → 插入 0x03），作用于整个 NAL（含 2 字节头） */
function ebspNal(nalHeader, rbsp) {
  const src = Buffer.concat([Buffer.from(nalHeader), rbsp])
  const out = []
  let zeros = 0
  for (const b of src) {
    if (zeros >= 2 && b <= 0x03) { out.push(0x03); zeros = 0 }
    out.push(b)
    zeros = (b === 0) ? zeros + 1 : 0
  }
  return Buffer.from(out)
}

/** 构造 HEVC Prefix_SEI NAL（nal_unit_type=39），内含一条 T.35 user data SEI */
function buildPrefixSeiNal(t35Buf) {
  return ebspNal([0x4E, 0x01], seiRbsp(t35Buf))
}
/** 构造 HEVC Suffix_SEI NAL（nal_unit_type=40） */
function buildSuffixSeiNal(t35Buf) {
  return ebspNal([0x50, 0x01], seiRbsp(t35Buf))
}

// ---------- 便捷向量（供验证） ----------

/** 最简单：仅默认参考白（无 HATM）→ 期望 00 00 */
function vectorMinimalDefault() {
  return encodeApplicationInfo({ colorVolumeTransform: { hasCustomHdrReferenceWhite: false, hasAdaptiveToneMap: false } })
}

/** 参考白配方（use_reference_white_tone_mapping=1），baselineHdrHeadroom 为 raw u16 */
function vectorReferenceWhiteRecipe(baselineHdrHeadroom) {
  return encodeApplicationInfo({
    colorVolumeTransform: {
      hasCustomHdrReferenceWhite: false,
      hasAdaptiveToneMap: true,
      adaptiveToneMap: { baselineHdrHeadroom, useReferenceWhiteToneMapping: true }
    }
  })
}

/** 自定义参考白（raw u16）+ 无 HATM */
function vectorCustomReferenceWhite(raw) {
  return encodeApplicationInfo({
    colorVolumeTransform: { hasCustomHdrReferenceWhite: true, hdrReferenceWhite: raw, hasAdaptiveToneMap: false }
  })
}

/** 显式 alternate 用例（type=0 分量混合 + PCHIP 增益曲线），用于编码/解码全路径自检 */
function vectorExplicitAlternate() {
  return encodeApplicationInfo({
    colorVolumeTransform: {
      hasCustomHdrReferenceWhite: true,
      hdrReferenceWhite: 100 * 3,    // raw（上层再按语义缩放）
      hasAdaptiveToneMap: true,
      adaptiveToneMap: {
        baselineHdrHeadroom: 2 * 10000,
        numAlternateImages: 1,
        chromaticitiesMode: 0,        // BT.709 常量
        hasCommonComponentMixParams: false,
        hasCommonCurveParams: false,
        alternates: [{
          headroom: 0,
          mixing: { type: 0 },        // 只用 Max
          curve: { ncp: 3, pchip: true, x: [0, 1000, 2000], y: [0, 0, 0] }
        }]
      }
    }
  })
}

module.exports = {
  T35_COUNTRY_US, T35_PROVIDER_SMPTE, T35_ORIENTED_APP5,
  encodeApplicationInfo, encodeColorVolumeTransform, encodeAdaptiveToneMap,
  encodeComponentMixing, encodeGainCurve,
  decodeApplicationInfo,
  t35Payload, buildPrefixSeiNal, buildSuffixSeiNal, seiRbsp,
  vectorMinimalDefault, vectorReferenceWhiteRecipe, vectorCustomReferenceWhite, vectorExplicitAlternate,
  hex
}
