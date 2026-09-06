// HDR 媒体实时预览模块 —— 移植自 hdr_preview/index.html 的媒体重渲染链（WebGL2 主链 + 2D CPU 回退）
// 只暴露「内容区」参数（输入传递函数 / 输入色域 / 目标渲染亮度）；输出端为固定 display-referred
// 显示模拟（extended-sRGB 直出 · display-p3 画布）——「渲染区参数留在 hdr_preview 工具内」。
// 定标语义（与上游 hdr-explorer 同源，参考白 203）：
//   PQ 输入 ×10000/203、HLG 输入 ×目标渲染亮度/203、SDR 输入 ×1；
//   曝光 EV 已移除（与转换「内容峰值亮度」联动重复），画面亮度/高光由 dispPeak（目标渲染亮度）调节。
// 与 hdr_preview 的差异（刻意）：不 clamp 输出上限（>1 = extended 高光，HDR 画布显示、
// SDR 8bit 画布硬件钳制）；无元数据解析 / 场景预览 / 曲线图（宿主用 ffprobe 探测）。
// 依赖：纯浏览器 API；WebGL2 不可用时自动回退 CPU 链。用法：
//   HdrPreviewMedia.init({ nativeCanvas, renderCanvas, badgeEl, playBtn, seekEl, timeEl, loopChk, frameScaleEl })
//   HdrPreviewMedia.loadMedia(src) / applyContent(tf, gamut, dispPeak) / refreshFrame() / destroy()
'use strict'

window.HdrPreviewMedia = (function () {
  /* =====================================================================
   * 数学核心（MATH-CORE，与 hdr_preview/index.html 一致，源自上游 color_functions.ts）
   * =================================================================== */
  function clamp01(x) { return x < 0 ? 0 : (x > 1 ? 1 : x) }
  function clamp(x, lo, hi) { return x < lo ? lo : (x > hi ? hi : x) }
  function matMul3(a, b) {
    const o = new Array(9)
    for (let r = 0; r < 3; r++)
      for (let c = 0; c < 3; c++)
        o[r * 3 + c] = a[r * 3] * b[c] + a[r * 3 + 1] * b[3 + c] + a[r * 3 + 2] * b[6 + c]
    return o
  }
  function matApply(m, v) {
    return [m[0] * v[0] + m[1] * v[1] + m[2] * v[2],
            m[3] * v[0] + m[4] * v[1] + m[5] * v[2],
            m[6] * v[0] + m[7] * v[1] + m[8] * v[2]]
  }
  function matInverse3(m) {
    const a = m[0], b = m[1], c = m[2], d = m[3], e = m[4], f = m[5], g = m[6], h = m[7], i = m[8]
    const A = e * i - f * h, B = -(d * i - f * h), C = d * h - e * g
    const D = -(b * i - c * h), E = a * i - c * g, F = -(a * h - b * g)
    const G = b * f - c * e, H = -(a * f - c * d), I = a * e - b * d
    const det = a * A + b * B + c * C
    return [A / det, D / det, G / det, B / det, E / det, H / det, C / det, F / det, I / det]
  }
  function rgbToXyzMatrix(c) { // c = [rx, ry, gx, gy, bx, by, wx, wy]
    if (c[0] === 1 && c[1] === 0 && c[2] === 0 && c[3] === 1 && c[4] === 0 && c[5] === 0) {
      return [1, 0, 0, 0, 1, 0, 0, 0, 1]
    }
    const rx = c[0], ry = c[1], gx = c[2], gy = c[3], bx = c[4], by = c[5], wx = c[6], wy = c[7]
    const oRx = (1 - rx) / ry, oGx = (1 - gx) / gy, oBx = (1 - bx) / by, oWx = (1 - wx) / wy
    const rq = rx / ry, gq = gx / gy, bq = bx / by, wq = wx / wy
    const bY = ((oWx - oRx) * (gq - rq) - (wq - rq) * (oGx - oRx)) /
               ((oBx - oRx) * (gq - rq) - (bq - rq) * (oGx - oRx))
    const gY = (wq - rq - bY * (bq - rq)) / (gq - rq)
    const rY = 1 - gY - bY
    const rS = rY / ry, gS = gY / gy, bS = bY / by
    return [rS * rx, gS * gx, bS * bx,
            rY,      gY,      bY,
            rS * (1 - rx - ry), gS * (1 - gx - gy), bS * (1 - bx - by)]
  }

  const TF_PQ = 'pq', TF_HLG = 'hlg', TF_SRGB = 'srgb', TF_REC709 = 'rec709',
        TF_G22 = 'g22', TF_G28 = 'g28', TF_REC2020_10 = 'rec2020_10',
        TF_REC2020_12 = 'rec2020_12', TF_LIN = 'lin'
  const TF_OPTIONS = [TF_PQ, TF_HLG, TF_SRGB, TF_REC709, TF_G22, TF_G28, TF_REC2020_10, TF_REC2020_12, TF_LIN]
  const MAX_NITS = {}
  MAX_NITS[TF_PQ] = 10000; MAX_NITS[TF_HLG] = 1000
  MAX_NITS[TF_SRGB] = 203; MAX_NITS[TF_REC709] = 203
  MAX_NITS[TF_G22] = 203;  MAX_NITS[TF_G28] = 203
  MAX_NITS[TF_REC2020_10] = 203; MAX_NITS[TF_REC2020_12] = 203
  MAX_NITS[TF_LIN] = 203
  const TF_NAMES = {}
  TF_NAMES[TF_PQ] = 'PQ'; TF_NAMES[TF_HLG] = 'HLG'; TF_NAMES[TF_SRGB] = 'sRGB'
  TF_NAMES[TF_REC709] = 'Rec.709'; TF_NAMES[TF_G22] = 'γ2.2'; TF_NAMES[TF_G28] = 'γ2.8'
  TF_NAMES[TF_REC2020_10] = 'Rec.2020 10bit'; TF_NAMES[TF_REC2020_12] = 'Rec.2020 12bit'
  TF_NAMES[TF_LIN] = '线性'

  const PQ_C1 = 107 / 128, PQ_C2 = 2413 / 128, PQ_C3 = 2392 / 128
  const PQ_M1 = 1305 / 8192, PQ_M2 = 2523 / 32
  const HLG_A = 0.17883277, HLG_B = 1 - 4 * HLG_A, HLG_C = 0.5 - HLG_A * Math.log(4 * HLG_A)

  function eotf(x, tf) {
    x = clamp01(x)
    if (tf === TF_G22) return Math.pow(x, 2.2)
    if (tf === TF_G28) return Math.pow(x, 2.8)
    if (tf === TF_SRGB || tf === TF_REC709 || tf === TF_REC2020_10 || tf === TF_REC2020_12) {
      if (x < 0.04045) return x / 12.92
      return Math.pow((x + 0.055) / 1.055, 2.4)
    }
    if (tf === TF_PQ) {
      const p = Math.pow(x, 1 / PQ_M2)
      return Math.pow(Math.max(p - PQ_C1, 0) / (PQ_C2 - PQ_C3 * p), 1 / PQ_M1)
    }
    if (tf === TF_HLG) {
      if (x <= 0.5) return x * x / 3
      return (Math.exp((x - HLG_C) / HLG_A) + HLG_B) / 12
    }
    return x
  }
  function oetf(x, tf) {
    x = clamp01(x)
    if (tf === TF_G22) return Math.pow(x, 1 / 2.2)
    if (tf === TF_G28) return Math.pow(x, 1 / 2.8)
    if (tf === TF_SRGB || tf === TF_REC709 || tf === TF_REC2020_10 || tf === TF_REC2020_12) {
      if (x < 0.003130800090713953) return 12.919999999992248 * x
      return Math.pow(1.1371188301409823 * x, 0.4166666666666667) - 0.05499994754780801
    }
    if (tf === TF_PQ) {
      const v = Math.pow(x, PQ_M1)
      return Math.pow((PQ_C1 + PQ_C2 * v) / (1 + PQ_C3 * v), PQ_M2)
    }
    if (tf === TF_HLG) {
      if (x < 1 / 12) return Math.sqrt(3 * x)
      return HLG_A * Math.log(12 * x - HLG_B) + HLG_C
    }
    return x
  }

  const GAMUTS = {
    '709':  { name: 'BT.709 / sRGB',      chroma: [0.6400, 0.3300, 0.3000, 0.6000, 0.1500, 0.0600, 0.3127, 0.3290] },
    '470m': { name: 'BT.470-6 System M',  chroma: [0.6700, 0.3300, 0.2100, 0.7100, 0.1400, 0.0800, 0.3100, 0.3160] },
    '470bg':{ name: 'BT.470-6 System BG', chroma: [0.6400, 0.3300, 0.2900, 0.6000, 0.1500, 0.0600, 0.3127, 0.3290] },
    '601':  { name: 'BT.601',             chroma: [0.6300, 0.3400, 0.3100, 0.5950, 0.1550, 0.0700, 0.3127, 0.3290] },
    '240m': { name: 'SMPTE 240M',         chroma: [0.6300, 0.3400, 0.3100, 0.5950, 0.1550, 0.0700, 0.3127, 0.3290] },
    'film': { name: 'Generic film',       chroma: [0.6810, 0.3190, 0.2430, 0.6920, 0.1450, 0.0490, 0.3100, 0.3160] },
    '2020': { name: 'BT.2020',            chroma: [0.7080, 0.2920, 0.1700, 0.7970, 0.1310, 0.0460, 0.3127, 0.3290] },
    'xyz':  { name: 'XYZ',                chroma: [1.0000, 0.0000, 0.0000, 1.0000, 0.0000, 0.0000, 0.3333, 0.3333] },
    '431':  { name: 'SMPTE RP 431-2',     chroma: [0.6800, 0.3200, 0.2650, 0.6900, 0.1500, 0.0600, 0.3140, 0.3510] },
    'p3':   { name: 'Display P3',         chroma: [0.6800, 0.3200, 0.2650, 0.6900, 0.1500, 0.0600, 0.3127, 0.3290] },
    '22':   { name: 'CICP 22',            chroma: [0.6300, 0.3400, 0.2950, 0.6050, 0.1550, 0.0770, 0.3127, 0.3290] },
  }
  const GAMUT_ORDER = ['709', '470m', '470bg', '601', '240m', 'film', '2020', 'xyz', '431', 'p3', '22']
  const XYZ_MAT = {}
  const XYZ_INV = {}
  for (const g in GAMUTS) {
    XYZ_MAT[g] = rgbToXyzMatrix(GAMUTS[g].chroma)
    XYZ_INV[g] = matInverse3(XYZ_MAT[g])
  }
  function gamutConvertMatrix(contentG, dispG) {
    if (contentG === dispG) return null
    return matMul3(XYZ_INV[dispG], XYZ_MAT[contentG])
  }

  function tonemap(x, mode) {
    if (mode === 'none') return clamp01(x)
    if (mode === 'reinhard') return clamp01(x / (1 + x))
    const x2 = Math.max(0, x - 0.004)
    return clamp01((x2 * (2.51 * x2 + 0.03)) / (x2 * (2.43 * x2 + 0.59) + 0.14))
  }
  function gamutMapSoft(r, g, b) {
    const m = Math.max(r, g, b)
    if (m <= 1) return [r, g, b]
    const s = 1 / (1 + 0.5 * (m - 1))
    return [r * s, g * s, b * s]
  }

  /* =====================================================================
   * GLSL（与 hdr_preview kColorFunctionGlsl 逐字一致；HDR_GL_FS 增 content_gain/disp_peak）
   * =================================================================== */
  const TF_TO_CICP = { pq: 16, hlg: 18, srgb: 13, rec709: 1, g22: 4, g28: 6, rec2020_10: 14, rec2020_12: 15, lin: 101 }
  const GAMUT_TO_CICP = { '709': 1, '470m': 4, '470bg': 5, '601': 6, '240m': 7, 'film': 8, '2020': 9, 'xyz': 10, '431': 11, 'p3': 12, '22': 22 }
  const DISPTF_TO_CICP = { srgb: 13, g24: 100 }

  const kColorFunctionGlsl = `
const int kPrimariesSRGB = 1;
const int kPrimariesBT470M = 4;
const int kPrimariesBT470BG = 5;
const int kPrimariesBT601 = 6;
const int kPrimariesSMPTE240 = 7;
const int kPrimariesGenericFilm = 8;
const int kPrimariesRec2020 = 9;
const int kPrimariesXYZ = 10;
const int kPrimariesSMPTE431 = 11;
const int kPrimariesP3 = 12;
const int kPrimaries22 = 22;

const int kTransferRec709 = 1;
const int kTransferG22 = 4;
const int kTransferG28 = 6;
const int kTransferSrgb = 13;
const int kTransferRec2020_10bit = 14;
const int kTransferRec2020_12bit = 15;
const int kTransferPQ = 16;
const int kTransferHLG = 18;
const int kTransferG24 = 100;   /* 附加：BT.1886 近似（纯 γ2.4） */
const int kTransferLinear = 101; /* 附加：线性 */

mat3 primariesToXYZD50(int primaries) {
  // Matrices are in column-major order.
  if (primaries == kPrimariesSRGB) {
    return mat3(0.43606567, 0.2224884,  0.01391602,
                0.38514709, 0.71687317, 0.09707642,
                0.14306641, 0.06060791, 0.71409607);
  }
  if (primaries == kPrimariesBT470M) {
    return mat3(0.63444409, 0.31099706, -0.00118351,
                0.1851249, 0.59148442, 0.0555158,
                0.144651, 0.09752533, 0.77087766);
  }
  if (primaries == kPrimariesBT470BG) {
    return mat3(0.45523212, 0.23228037, 0.01453973,
                0.36758438, 0.7078171, 0.1049015,
                0.1414035, 0.05990934, 0.70576872);
  }
  if (primaries == kPrimariesBT601) {
    return mat3(0.41627875, 0.22167341, 0.01365182,
                0.39318293, 0.70327732, 0.09134889,
                0.15475831, 0.07505608, 0.72020924);
  }
  if (primaries == kPrimariesSMPTE240) {
    return mat3(0.41627875, 0.22167341, 0.01365182,
                0.39318293, 0.70327732, 0.09134889,
                0.15475831, 0.07505608, 0.72020924);
  }
  if (primaries == kPrimariesGenericFilm) {
    return mat3(0.56563546, 0.26423991, -0.00132302,
                0.25386551, 0.68507162, 0.05499217,
                0.14471901, 0.05069527, 0.7715408);
  }
  if (primaries == kPrimariesRec2020) {
    return mat3(0.673459,  0.279033,   -0.00193139,
                0.165661,  0.675338,    0.0299794,
                0.1251,    0.0456288,   0.797162);
  }
  if (primaries == kPrimariesXYZ) {
    return mat3(0.99779433, -0.0097391, -0.0074265,
                -0.00414498, 1.01831505, 0.01345748,
                -0.02942053, -0.00856657, 0.81893327);
  }
  if (primaries == kPrimariesSMPTE431) {
    return mat3(0.4861451, 0.22668035, -0.00080052,
                0.32383739, 0.7103286, 0.04323842,
                0.15423751, 0.06299786, 0.78277204);
  }
  if (primaries == kPrimariesP3) {
    return mat3(0.515102,  0.241182,  -0.00104941,
                0.291965,  0.692236,   0.0418818,
                0.157153,  0.0665819,  0.784378);
  }
  if (primaries == kPrimaries22) {
    return mat3(0.45425407, 0.24189572, 0.01489721,
                0.35330461, 0.67364839, 0.09064625,
                0.1566613, 0.0844627, 0.71966648);
  }
  return mat3(1.0);
}
mat3 primariesFromXYZD50(int primaries) {
  // Matrices are in column-major order.
  if (primaries == kPrimariesSRGB) {
    return mat3( 3.13411215, -0.97878729,  0.07198304,
                -1.61739246,  1.91627959, -0.22898585,
                -0.4906334,   0.03345471,  1.40538513);
  }
  if (primaries == kPrimariesBT470M) {
    return mat3(1.84618387, -0.98284434, 0.07361526,
                -0.55186312, 2.00477096, -0.14522355,
                -0.27660901, -0.06920234, 1.30178174);
  }
  if (primaries == kPrimariesBT470BG) {
    return mat3(2.96076028, -0.97876671, 0.08448299,
                -1.46814038, 1.91613139, -0.25455746,
                -0.46857635, 0.03344844, 1.42157643);
  }
  if (primaries == kPrimariesBT601) {
    return mat3(3.39269187, -1.07709506, 0.07230543,
                -1.82679596, 2.0213881, -0.22175845,
                -0.53864223, 0.02078832, 1.39605881);
  }
  if (primaries == kPrimariesSMPTE240) {
    return mat3(3.39269187, -1.07709506, 0.07230543,
                -1.82679596, 2.0213881, -0.22175845,
                -0.53864223, 0.02078832, 1.39605881);
  }
  if (primaries == kPrimariesGenericFilm) {
    return mat3(2.12127665, -0.82280953, 0.0622839,
                -0.75813607, 1.76151021, -0.12685303,
                -0.34807641, 0.03859283, 1.29276013);
  }
  if (primaries == kPrimariesRec2020) {
    return mat3( 1.6472752,  -0.68261762,  0.02966273,
                -0.39360248,  1.64761778, -0.06291669,
                -0.23598029,  0.01281627,  1.25339643);
  }
  if (primaries == kPrimariesXYZ) {
    return mat3(1.00251407, 0.00966312, 0.00893251,
                0.00360421, 0.98191337, -0.01610303,
                0.03605345, 0.01061859, 1.22125318);
  }
  if (primaries == kPrimariesSMPTE431) {
    return mat3(2.59636133, -0.83286754, 0.0486608,
                -1.15820908, 1.78626314, -0.0998533,
                -0.41837417, 0.02034904, 1.27595925);
  }
  if (primaries == kPrimariesP3) {
    return mat3( 2.40404516, -0.84222838,  0.04818706,
                -0.98989869,  1.79885051, -0.09737385,
                -0.39763172,  0.01604817,  1.27350664);
  }
  if (primaries == kPrimaries22) {
    return mat3(3.02857116, -1.09697212, 0.07547822,
                -1.52372877, 2.06017976, -0.22795041,
                -0.48044708, -0.0029948, 1.3998551);
  }
  return mat3(1.0);
}
vec3 primariesConvert(vec3 rgb, int src, int dst) {
  if (src == dst) {
    return rgb;
  }
  return primariesFromXYZD50(dst) * primariesToXYZD50(src) * rgb;
}

float transferToLinear(float x, int transfer) {
  if (transfer == kTransferRec709 ||
      transfer == kTransferRec2020_10bit ||
      transfer == kTransferRec2020_12bit) {
    transfer = kTransferSrgb;
  }
  if (transfer == kTransferG22) {
    return pow(x, 2.2);
  }
  if (transfer == kTransferG28) {
    return pow(x, 2.8);
  }
  if (transfer == kTransferG24) {
    return pow(x, 2.4);
  }
  if (transfer == kTransferLinear) {
    return x;
  }
  if (transfer == kTransferSrgb) {
    if (x < 0.04045)
      return x / 12.92;
    return pow((x + 0.055)/1.055, 2.4);
  }
  if (transfer == kTransferPQ) {
    float c1 =  107.0 / 128.0;
    float c2 = 2413.0 / 128.0;
    float c3 = 2392.0 / 128.0;
    float m1 = 1305.0 / 8192.0;
    float m2 = 2523.0 / 32.0;
    float p = pow(clamp(x, 0.0, 1.0), 1.0 / m2);
    return pow(max(p - c1, 0.0) / (c2 - c3 * p), 1.0 / m1);
  }
  if (transfer == kTransferHLG) {
    const float a = 0.17883277;
    const float b = 1.0 - 4.0 * a;
    const float c = 0.5 - a * log(4.0 * a);
    if (x <= 0.5) {
      return x * x / 3.0;
    } else {
      return (exp((x - c) / a) + b) / 12.0;
    }
  }
  return 0.0;
}
float transferFromLinear(float x, int transfer) {
  if (transfer == kTransferRec709 ||
      transfer == kTransferRec2020_10bit ||
      transfer == kTransferRec2020_12bit) {
    transfer = kTransferSrgb;
  }
  if (transfer == kTransferG22) {
    return pow(x, 1.0/2.2);
  }
  if (transfer == kTransferG28) {
    return pow(x, 1.0/2.8);
  }
  if (transfer == kTransferG24) {
    return pow(x, 1.0/2.4);
  }
  if (transfer == kTransferLinear) {
    return x;
  }
  if (transfer == kTransferSrgb) {
    if (x < 0.003130800090713953)
      return 12.919999999992248*x;
    return pow(1.1371188301409823*x, 0.4166666666666667) - 0.05499994754780801;
  }
  if (transfer == kTransferPQ) {
    float c1 =  107.0 / 128.0;
    float c2 = 2413.0 / 128.0;
    float c3 = 2392.0 / 128.0;
    float m1 = 1305.0 / 8192.0;
    float m2 = 2523.0 / 32.0;
    float v = pow(clamp(x, 0.0, 1.0), m1);
    return pow((c1 + c2 * v) / (1.0 + c3 * v), m2);
  }
  if (transfer == kTransferHLG) {
    const float a = 0.17883277;
    const float b = 1.0 - 4.0*a;
    const float c = 0.5 - a * log(4.0 * a);
    if (x < 1.0/12.0) {
      return sqrt(3.0 * x);
    }
    return a * log(12.0 * x - b) + c;
  }
  return clamp(x, 0.0, 1.0);
}

vec3 ApplyOetfInv(vec3 x, int transfer) {
  return vec3(sign(x[0]) * transferToLinear(abs(x[0]), transfer),
              sign(x[1]) * transferToLinear(abs(x[1]), transfer),
              sign(x[2]) * transferToLinear(abs(x[2]), transfer));
}
vec3 ApplyOetf(vec3 x, int transfer) {
  return vec3(sign(x[0]) * transferFromLinear(abs(x[0]), transfer),
              sign(x[1]) * transferFromLinear(abs(x[1]), transfer),
              sign(x[2]) * transferFromLinear(abs(x[2]), transfer));
}

vec3 ToDisplayWithClamping(vec3 rgb, int texture_primaries, int framebuffer_primaries,
                       int framebuffer_trfn, float target_log2_headroom,
                       float linear_scale, bool show_clamped) {
  float max_val = exp2(target_log2_headroom);
  if (show_clamped && (rgb.r > max_val || rgb.g > max_val || rgb.b > max_val)) {
    return vec3(1.0, 0.0, 1.0); // Bright pink
  }
  vec3 rgb_display = primariesConvert(rgb, texture_primaries, framebuffer_primaries);
  if (show_clamped && (rgb_display.r > max_val || rgb_display.g > max_val || rgb_display.b > max_val)) {
    return vec3(0.0, 1.0, 1.0); // Bright cyan
  }
  rgb_display *= linear_scale;
  rgb_display = ApplyOetf(rgb_display, framebuffer_trfn);
  return clamp(rgb_display, 0.0, max_val);
}

float log10(float x) { return log(x) / log(10.0); }

/* HLG OOTF（BT.2100，γ 随显示峰值自适应，BT.2408 Note 5f；继承上游写法） */
vec3 ApplyOotfAdaptiveHlg(vec3 rgb, int texture_trfn, float presentation_display_peak_luminance) {
  if (texture_trfn != kTransferHLG) {
    return rgb;
  }
  rgb = primariesConvert(rgb, texture_primaries, kPrimariesRec2020);
  float Y = 0.2627 * rgb.r + 0.6780 * rgb.g + 0.0593 * rgb.b;
  float L_W = presentation_display_peak_luminance;
  float gamma;
  if (L_W >= 400.0 || L_W <= 2000.0) {
    gamma = 1.2 + 0.42 * log10(L_W / 1000.0);
  } else {
    gamma = 1.2 * pow(1.111, log2(L_W / 1000.0));
  }
  rgb *= pow(Y, gamma - 1.0);
  rgb = primariesConvert(rgb, kPrimariesRec2020, texture_primaries);
  return rgb;
}
`
  const HDR_GL_VS = `#version 300 es
precision highp float;
in vec2 position;
out vec2 texcoord;
void main() {
  vec2 uv = vec2(0.5 + 0.5 * position.x, 0.5 - 0.5 * position.y);
  texcoord = uv;
  gl_Position = vec4(position, 0.0, 1.0);
}`

  const HDR_GL_FS = `#version 300 es
precision highp float;
uniform sampler2D content;
uniform int texture_trfn;
uniform int texture_primaries;
uniform float content_gain;
uniform int framebuffer_primaries;
uniform float disp_peak;
uniform int out_pq;
in vec2 texcoord;
out vec4 fragColor;
` + kColorFunctionGlsl + `
void main() {
  vec3 rgb = texture(content, texcoord).rgb;
  // ① 输入解码（内容区「输入传递函数」；相对内容峰值光，与上游语义一致）
  rgb = ApplyOetfInv(rgb, texture_trfn);
  // ② HLG 内容端 OOTF（仅 HLG 输入；L_W = 目标渲染亮度 disp_peak，同 hdr_preview）
  rgb = ApplyOotfAdaptiveHlg(rgb, texture_trfn, disp_peak);
  // ③ 定标（参考白 203，同 hdr_preview）：PQ ×内容峰值/203；HLG ×目标渲染亮度/203；SDR ×1。
  //    （曝光 EV 已移除；亮度/高光由 disp_peak 调节）
  rgb *= content_gain;
  // ④ 内容色域 → 显示色域（display-p3：画布 drawingBufferColorSpace）
  rgb = primariesConvert(rgb, texture_primaries, framebuffer_primaries);
  // ⑤ 显示编码：extended-sRGB 直出
  float max_val = disp_peak / 203.0;
  if (out_pq == 1) {
    // PQ（HDR10 导出）呈现：显示端软膝色调映射（tanh 软滚降）——高光平滑收束到峰值，
    // 替代「硬钳 max_val」造成的大片死白/过曝感；内容在 PQ 编码域呈现（同导出文件）
    rgb = vec3(max_val * tanh(rgb.x / max_val),
               max_val * tanh(rgb.y / max_val),
               max_val * tanh(rgb.z / max_val));
    fragColor.rgb = ApplyOetf(rgb, kTransferSrgb);
  } else {
    fragColor.rgb = clamp(ApplyOetf(rgb, kTransferSrgb), 0.0, max_val);
  }
  fragColor.a = 1.0;
}`

  /* =====================================================================
   * WebGL2 渲染器（MediaGlRenderer，与 hdr_preview 一致；setParams 增加 gain/disp_peak）
   * =================================================================== */
  function compileGlShader(gl, type, source) {
    const sh = gl.createShader(type)
    gl.shaderSource(sh, source)
    gl.compileShader(sh)
    if (!gl.getShaderParameter(sh, gl.COMPILE_STATUS)) {
      const log = gl.getShaderInfoLog(sh) || ''
      throw new Error('GLSL 编译失败: ' + log)
    }
    return sh
  }
  function compileGlProgram(gl, vsSource, fsSource) {
    const vs = compileGlShader(gl, gl.VERTEX_SHADER, vsSource)
    const fs = compileGlShader(gl, gl.FRAGMENT_SHADER, fsSource)
    const program = gl.createProgram()
    gl.attachShader(program, vs)
    gl.attachShader(program, fs)
    gl.linkProgram(program)
    if (!gl.getProgramParameter(program, gl.LINK_STATUS)) {
      throw new Error('GLSL 链接失败: ' + (gl.getProgramInfoLog(program) || ''))
    }
    gl.deleteShader(vs)
    gl.deleteShader(fs)
    return program
  }
  class MediaGlRenderer {
    constructor(canvas) {
      this.canvas = canvas
      this.gl = canvas.getContext('webgl2', { antialias: false, depth: false, stencil: false })
      if (!this.gl) throw new Error('浏览器不支持 WebGL2')
      const gl = this.gl
      gl.getExtension('EXT_color_buffer_half_float')
      gl.getExtension('EXT_color_buffer_float')
      this.hdrCanvas = false
      try {
        if (canvas.configureHighDynamicRange) {
          canvas.configureHighDynamicRange({ mode: 'extended' })
          this.hdrCanvas = true
        }
      } catch (e) { this.hdrCanvas = false }
      try { gl.drawingBufferColorSpace = 'display-p3' } catch (e) { /* ignore */ }
      this.program = compileGlProgram(gl, HDR_GL_VS, HDR_GL_FS)
      this.tex = null
      this.texW = 0
      this.texH = 0
      this.texFloat = true
      this.vb = null
      this.ib = null
    }
    resize(w, h) {
      const gl = this.gl
      if (this.canvas.width !== w) this.canvas.width = w
      if (this.canvas.height !== h) this.canvas.height = h
      if (this.hdrCanvas && gl.drawingBufferStorage) {
        try { gl.drawingBufferStorage(gl.RGBA16F, w, h) } catch (e) { /* ignore */ }
      }
      gl.viewport(0, 0, w, h)
    }
    setImage(bitmap) {
      const gl = this.gl
      if (!this.tex) this.tex = gl.createTexture()
      gl.bindTexture(gl.TEXTURE_2D, this.tex)
      gl.pixelStorei(gl.UNPACK_COLORSPACE_CONVERSION_WEBGL, gl.NONE)
      const w = bitmap.width, h = bitmap.height
      const realloc = this.texW !== w || this.texH !== h
      if (realloc) {
        this.texW = w
        this.texH = h
        try {
          gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA16F, w, h, 0, gl.RGBA, gl.FLOAT, null)
          this.texFloat = true
        } catch (e) {
          gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA8, w, h, 0, gl.RGBA, gl.UNSIGNED_BYTE, null)
          this.texFloat = false
        }
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE)
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE)
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR)
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR)
      }
      if (this.texFloat) {
        try { gl.texSubImage2D(gl.TEXTURE_2D, 0, 0, 0, gl.RGBA, gl.FLOAT, bitmap) }
        catch (e) { gl.texSubImage2D(gl.TEXTURE_2D, 0, 0, 0, gl.RGBA, gl.UNSIGNED_BYTE, bitmap) }
      } else {
        gl.texSubImage2D(gl.TEXTURE_2D, 0, 0, 0, gl.RGBA, gl.UNSIGNED_BYTE, bitmap)
      }
      gl.bindTexture(gl.TEXTURE_2D, null)
    }
    setParams(st) {
      this.params = st
    }
    draw() {
      const gl = this.gl
      if (gl.isContextLost && gl.isContextLost()) return
      if (!this.tex) {
        gl.clearColor(0.25, 0.25, 0.25, 1)
        gl.clear(gl.COLOR_BUFFER_BIT)
        return
      }
      const st = this.params
      gl.viewport(0, 0, this.canvas.width, this.canvas.height)
      gl.useProgram(this.program)
      gl.clearColor(0.25, 0.25, 0.25, 1)
      gl.clear(gl.COLOR_BUFFER_BIT)
      if (!this.vb) {
        this.vb = gl.createBuffer()
        gl.bindBuffer(gl.ARRAY_BUFFER, this.vb)
        gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 1, -1, 1, 1, -1, 1]), gl.STATIC_DRAW)
        this.ib = gl.createBuffer()
        gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, this.ib)
        gl.bufferData(gl.ELEMENT_ARRAY_BUFFER, new Uint16Array([0, 1, 2, 0, 2, 3]), gl.STATIC_DRAW)
      }
      gl.bindBuffer(gl.ARRAY_BUFFER, this.vb)
      gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, this.ib)
      const posLoc = gl.getAttribLocation(this.program, 'position')
      gl.vertexAttribPointer(posLoc, 2, gl.FLOAT, false, 0, 0)
      gl.enableVertexAttribArray(posLoc)
      gl.activeTexture(gl.TEXTURE0)
      gl.bindTexture(gl.TEXTURE_2D, this.tex)
      const uloc = name => gl.getUniformLocation(this.program, name)
      gl.uniform1i(uloc('content'), 0)
      gl.uniform1i(uloc('texture_trfn'), TF_TO_CICP[st.tf] !== undefined ? TF_TO_CICP[st.tf] : 13)
      gl.uniform1i(uloc('texture_primaries'), GAMUT_TO_CICP[st.gamut] !== undefined ? GAMUT_TO_CICP[st.gamut] : 9)
      gl.uniform1f(uloc('content_gain'), st.gain)
      gl.uniform1i(uloc('framebuffer_primaries'), 12) // display-p3 画布
      gl.uniform1f(uloc('disp_peak'), st.dispPeak)
      gl.uniform1i(uloc('out_pq'), st.outTf ? 1 : 0)
      gl.drawElements(gl.TRIANGLES, 6, gl.UNSIGNED_SHORT, 0)
    }
  }

  /* =====================================================================
   * 渲染状态（内容区可调：输入 TF/色域/目标渲染亮度；输出端为 display-referred 显示模拟：
   * extended-sRGB 直出 + display-p3 画布。Chromium 画布不能直接写 PQ 信号，
   * 转换产物（PQ 码值）由转换完成后 <video> 播放真文件验证）
   * =================================================================== */
  const state = { tf: 'srgb', gamut: '709', dispPeak: 500, ootf: true, lutN: 4096, outTf: null }
  let currentState = null

  function buildState() {
    const st = Object.assign({}, state)
    st.maxNits = MAX_NITS[state.tf] || 203          // 输入峰值（仅信息用）
    st.dispPeak = state.dispPeak || 400             // 目标渲染亮度（hdr_preview 显示峰值滑块同义）
    st.outTf = state.outTf || null                  // Eclipsa：'pq'|'hlg'（输出编码传函）→ 内容按所选传函导出呈现
    // 定标增益（参考白 203，上游语义）：PQ ×内容峰值/参考白；HLG ×目标渲染亮度/参考白；SDR ×1。
    // 曝光 EV 已移除（与转换「内容峰值亮度」联动重复；亮度/高光由 dispPeak 调节）
    st.gain = (state.tf === TF_PQ ? 10000 / 203 : state.tf === TF_HLG ? st.dispPeak / 203 : 1)
    st.ootf = state.ootf && state.tf === TF_HLG      // HLG 内容端 OOTF（默认开）
    const n = state.lutN
    st.lutN = n
    const oeLut = new Float32Array(n)
    const eoLut = new Float32Array(n)
    for (let i = 0; i < n; i++) {
      const v = (i + 0.5) / n
      oeLut[i] = oetf(v, state.tf)
      eoLut[i] = eotf(v, state.tf)
    }
    st.oeLut = oeLut
    st.eoLut = eoLut
    st.conv = gamutConvertMatrix(state.gamut, 'p3')  // 内容 → 显示色域（display-p3 画布）
    st.to2020 = state.gamut !== '2020' ? matMul3(XYZ_INV['2020'], XYZ_MAT[state.gamut]) : null
    st.from2020 = matMul3(XYZ_INV['p3'], XYZ_MAT['2020'])
    return st
  }

  /** sRGB OETF（不做 clamp01——>1 为 extended 高光，交给画布/HDR 显示；负值钳 0 防偏色） */
  function oetfSrgbRaw(x) {
    x = Math.max(0, x)
    if (x < 0.003130800090713953) return 12.919999999992248 * x
    return Math.pow(1.1371188301409823 * x, 0.4166666666666667) - 0.05499994754780801
  }

  /** 内容码值 → 显示码值（上游定标语义：解码 → OOTF → ×增益 → 色域 → extended-sRGB 直出，
   *  钳制到目标渲染亮度 headroom（dispPeak/203）——与 hdr_preview 媒体链一致） */
  function decodeAndRenderCode(cr, cg, cb, st) {
    const lutN = st.lutN
    let lr = st.eoLut[Math.min(lutN - 1, (cr * lutN) | 0)]
    let lg = st.eoLut[Math.min(lutN - 1, (cg * lutN) | 0)]
    let lb = st.eoLut[Math.min(lutN - 1, (cb * lutN) | 0)]
    if (st.ootf) {
      let q = st.to2020 ? matApply(st.to2020, [lr, lg, lb]) : [lr, lg, lb]
      const Y = 0.2627 * q[0] + 0.678 * q[1] + 0.0593 * q[2]
      const p = Math.pow(Math.max(Y, 0), 0.2)
      q = [q[0] * p, q[1] * p, q[2] * p]
      if (st.from2020) q = matApply(st.from2020, q)
      lr = q[0]; lg = q[1]; lb = q[2]
    }
    lr *= st.gain; lg *= st.gain; lb *= st.gain
    if (st.conv) {
      const q = matApply(st.conv, [lr, lg, lb])
      lr = q[0]; lg = q[1]; lb = q[2]
    }
    const maxV = st.dispPeak / 203
    if (st.outTf) {
      // PQ/HLG（Eclipsa 输出呈现，CPU 回退链）：显示端软膝色调映射（tanh 软滚降），
      // 高光平滑收束到峰值，替代硬钳 maxV 的大片死白/过曝
      const knee = (x) => maxV * Math.tanh(x / maxV)
      return [
        oetfSrgbRaw(knee(lr)),
        oetfSrgbRaw(knee(lg)),
        oetfSrgbRaw(knee(lb)),
      ]
    }
    return [
      Math.min(maxV, oetfSrgbRaw(lr)),
      Math.min(maxV, oetfSrgbRaw(lg)),
      Math.min(maxV, oetfSrgbRaw(lb)),
    ]
  }

  /* =====================================================================
   * 媒体元素 / 帧泵 / GL 初始化（移植 hdr_preview media 段）
   * =================================================================== */
  const mediaVideo = document.createElement('video')
  mediaVideo.muted = true
  mediaVideo.playsInline = true
  mediaVideo.preload = 'auto'
  mediaVideo.style.display = 'none'
  document.body.appendChild(mediaVideo)

  let nativeCv = null       // 原生参考画布
  let renderCv = null       // 重渲染画布
  let badgeEl = null
  let mirrorVideo = null    // 宿主源视频（播放/暂停/进度镜像）
  let mediaGl = null
  let mediaGlFailed = false
  let mediaGlBusy = false
  let mediaGlError = ''
  let mediaPumpRaf = null
  let mediaLastTime = -1
  let srcType = null        // 'video' | null
  let destroyed = false
  let glFrameLogged = false
  let cpuFrameLogged = false
  const mediaOffCv = document.createElement('canvas')
  const mediaOffCtx = mediaOffCv.getContext('2d')

  function updateBadge() {
    if (!badgeEl) return
    const out = state.outTf
      ? (state.outTf === 'hlg' ? 'HLG' : 'PQ（HDR10）') + ' 导出呈现 · 软膝色调映射 · '
      : ''
    const chain = out + '峰值 ' + (state.dispPeak || 500) + ' nits · extended-sRGB 直出'
    if (mediaGlFailed || !mediaGl) {
      badgeEl.textContent = chain + ' · CPU 渲染' + (mediaGlError ? '（' + mediaGlError + '）' : '')
      badgeEl.style.color = '#f28b82'
    } else if (mediaGl.hdrCanvas) {
      badgeEl.textContent = chain + ' · WebGL2 · HDR 画布(extended)'
      badgeEl.style.color = '#7cb342'
    } else {
      badgeEl.textContent = chain + ' · WebGL2 · SDR 画布'
      badgeEl.style.color = '#7cb342'
    }
  }

  function initGl() {
    if (mediaGl || mediaGlFailed || destroyed || !renderCv) return
    try {
      mediaGl = new MediaGlRenderer(renderCv)
      mediaGl.setParams(currentState)
    } catch (e) {
      mediaGl = null
      mediaGlFailed = true
      mediaGlError = String((e && e.message) || e).replace(/\s+/g, ' ').slice(0, 90)
      console.warn('[hdr-preview] WebGL2 初始化失败，回退 CPU：', e)
    }
    updateBadge()
  }

  async function pumpFrameGl() {
    if (mediaGlBusy || !mediaGl || mediaGlFailed || destroyed) return
    if (!mediaVideo.videoWidth || !mediaVideo.videoHeight) return
    const maxW = parseInt(frameScaleEl ? frameScaleEl.value : '720', 10) || 720
    const scale = Math.min(1, maxW / mediaVideo.videoWidth)
    const w = Math.max(2, Math.round(mediaVideo.videoWidth * scale))
    const h = Math.max(2, Math.round(mediaVideo.videoHeight * scale))
    if (nativeCv) {
      nativeCv.width = w
      nativeCv.height = h
      nativeCv.getContext('2d').drawImage(mediaVideo, 0, 0, w, h)
    }
    mediaGlBusy = true
    try {
      let bitmap = null
      try {
        bitmap = await createImageBitmap(mediaVideo, { colorSpaceConversion: 'none' })
      } catch (e) {
        mediaOffCv.width = w
        mediaOffCv.height = h
        mediaOffCtx.drawImage(mediaVideo, 0, 0, w, h)
      }
      if (destroyed || !mediaGl) return
      mediaGl.resize(w, h)
      if (bitmap) {
        mediaGl.setImage(bitmap)
        if (bitmap.close) try { bitmap.close() } catch (e) { /* ignore */ }
      } else {
        mediaGl.setImage(mediaOffCv)
      }
      mediaGl.setParams(currentState)
      mediaGl.draw()
      if (!glFrameLogged) {
        glFrameLogged = true
        console.info('[hdr-preview] WebGL2 渲染就绪 ' + w + 'x' + h + (mediaGl.hdrCanvas ? ' · HDR 画布(extended)' : ' · SDR 画布'))
      }
    } catch (e) {
      console.warn('[hdr-preview] GL 帧更新失败，回退 CPU：', e)
      mediaGl = null
      mediaGlFailed = true
      mediaGlError = String((e && e.message) || e).replace(/\s+/g, ' ').slice(0, 90)
      updateBadge()
      renderFrameCpu()
    } finally {
      mediaGlBusy = false
    }
  }

  function renderFrameCpu() {
    if (destroyed || !mediaVideo.videoWidth || !mediaVideo.videoHeight || !renderCv) return
    const st = currentState
    const maxW = parseInt(frameScaleEl ? frameScaleEl.value : '720', 10) || 720
    const scale = Math.min(1, maxW / mediaVideo.videoWidth)
    const w = Math.max(2, Math.round(mediaVideo.videoWidth * scale))
    const h = Math.max(2, Math.round(mediaVideo.videoHeight * scale))
    if (nativeCv) {
      nativeCv.width = w
      nativeCv.height = h
      nativeCv.getContext('2d').drawImage(mediaVideo, 0, 0, w, h)
    }
    mediaOffCv.width = w
    mediaOffCv.height = h
    mediaOffCtx.drawImage(mediaVideo, 0, 0, w, h)
    const px = mediaOffCtx.getImageData(0, 0, w, h).data
    renderCv.width = w
    renderCv.height = h
    const rctx = renderCv.getContext('2d')
    const rimg = rctx.createImageData(w, h)
    const rpx = rimg.data
    for (let i = 0; i < px.length; i += 4) {
      const out = decodeAndRenderCode(px[i] / 255, px[i + 1] / 255, px[i + 2] / 255, st)
      rpx[i] = out[0] * 255
      rpx[i + 1] = out[1] * 255
      rpx[i + 2] = out[2] * 255
      rpx[i + 3] = 255
    }
    rctx.putImageData(rimg, 0, 0)
    if (!cpuFrameLogged) {
      cpuFrameLogged = true
      console.info('[hdr-preview] CPU 链渲染 ' + w + 'x' + h)
    }
  }

  function renderFrame() {
    if (mediaGl && !mediaGlFailed) { pumpFrameGl(); return }
    renderFrameCpu()
  }

  function startPump() {
    if (mediaPumpRaf !== null) return
    const loop = () => {
      if (!destroyed && mediaVideo.readyState >= 2) {
        const t = mediaVideo.currentTime
        if (Math.abs(t - mediaLastTime) > 0.001) {
          mediaLastTime = t
          renderFrame()
        }
      }
      mediaPumpRaf = requestAnimationFrame(loop)
    }
    mediaPumpRaf = requestAnimationFrame(loop)
  }
  function stopPump() {
    if (mediaPumpRaf !== null) { cancelAnimationFrame(mediaPumpRaf); mediaPumpRaf = null }
  }

  /* =====================================================================
   * 传输控件（同一播放控件驱动 重渲染画布 + 镜像源视频）
   * =================================================================== */
  let playBtn = null
  let seekEl = null
  let timeEl = null
  let loopChk = null
  let frameScaleEl = null
  let seekingFromMirror = false
  let wired = false
  let mirrorWired = null

  function fmtTime(s) {
    if (!isFinite(s)) return '0:00'
    s = Math.max(0, Math.floor(s))
    return Math.floor(s / 60) + ':' + String(s % 60).padStart(2, '0')
  }
  function updatePlayBtn() {
    if (playBtn) playBtn.textContent = mediaVideo.paused ? '▶ 播放' : '⏸ 暂停'
  }
  function updateTime() {
    if (seekEl && mediaVideo.duration) {
      seekEl.value = String(Math.round(mediaVideo.currentTime / mediaVideo.duration * 1000))
    }
    if (timeEl) timeEl.textContent = fmtTime(mediaVideo.currentTime) + ' / ' + fmtTime(mediaVideo.duration)
  }

  function wireControls() {
    if (wired) return // 元素持久：init/destroy 循环不重复绑定
    wired = true
    if (!playBtn || !seekEl) return
    playBtn.addEventListener('click', () => {
      if (mediaVideo.paused) { mediaVideo.play() } else { mediaVideo.pause() }
    })
    mediaVideo.addEventListener('play', updatePlayBtn)
    mediaVideo.addEventListener('pause', updatePlayBtn)
    mediaVideo.addEventListener('ended', updatePlayBtn)
    mediaVideo.addEventListener('loadedmetadata', () => {
      if (seekEl) seekEl.value = '0'
      updateTime()
    })
    mediaVideo.addEventListener('timeupdate', () => {
      updateTime()
      if (loopChk && loopChk.checked && mediaVideo.ended) {
        mediaVideo.currentTime = 0
        mediaVideo.play()
      }
    })
    seekEl.addEventListener('input', () => {
      if (!mediaVideo.duration) return
      mediaVideo.currentTime = seekEl.value / 1000 * mediaVideo.duration
    })
    if (loopChk) loopChk.addEventListener('change', () => { mediaVideo.loop = loopChk.checked })
    if (frameScaleEl) frameScaleEl.addEventListener('input', () => { if (srcType) renderFrame() })

    // 镜像：本模块为主控 → 源视频跟随
    mediaVideo.addEventListener('play', () => {
      if (!mirrorVideo || mirrorVideo === mediaVideo) return
      if (mirrorVideo.paused) mirrorVideo.play().catch(() => { /* ignore */ })
    })
    mediaVideo.addEventListener('pause', () => {
      if (!mirrorVideo || mirrorVideo === mediaVideo) return
      try { mirrorVideo.pause() } catch (e) { /* ignore */ }
    })
    mediaVideo.addEventListener('seeked', () => {
      if (!mirrorVideo || mirrorVideo === mediaVideo || seekingFromMirror) return
      if (Math.abs(mirrorVideo.currentTime - mediaVideo.currentTime) > 0.1) {
        seekingFromMirror = true
        mirrorVideo.currentTime = mediaVideo.currentTime
        setTimeout(() => { seekingFromMirror = false }, 120)
      }
    })
  }

  // 反向镜像：源视频（宿主原生控件）播放/暂停/拖动 → 本模块跟随
  function bindMirror(el) {
    if (mirrorWired === el) return
    mirrorWired = el
    mirrorVideo = el
    if (!mirrorVideo || mirrorVideo === mediaVideo) return
    mirrorVideo.addEventListener('play', () => { if (mediaVideo.paused) mediaVideo.play() })
    mirrorVideo.addEventListener('pause', () => { if (!mediaVideo.paused) mediaVideo.pause() })
    mirrorVideo.addEventListener('seeked', () => {
      if (seekingFromMirror) return
      if (Math.abs(mediaVideo.currentTime - mirrorVideo.currentTime) > 0.1) {
        seekingFromMirror = true
        mediaVideo.currentTime = mirrorVideo.currentTime
        setTimeout(() => { seekingFromMirror = false }, 120)
      }
    })
  }

  // 参数变化：重算状态并重绘（暂停时也立即刷新当前帧）
  let renderQueued = false
  function scheduleRender() {
    currentState = buildState()
    updateBadge()
    if (mediaGl && !mediaGlFailed) {
      try {
        mediaGl.setParams(currentState)
        mediaGl.draw()
      } catch (e) { /* ignore */ }
    }
    if (renderQueued) return
    renderQueued = true
    requestAnimationFrame(() => {
      renderQueued = false
      if (!mediaVideo.paused || mediaVideo.readyState >= 2) renderFrame()
    })
  }

  /* =====================================================================
   * 公开 API
   * =================================================================== */
  return {
    /**
     * 挂接 DOM 与传输控件（视图由宿主构建，元素直接传入）。
     * opts: { nativeCanvas, renderCanvas, badgeEl, playBtn, seekEl, timeEl, loopChk, frameScaleEl, mirrorVideo }
     */
    init(opts) {
      nativeCv = opts.nativeCanvas || null
      renderCv = opts.renderCanvas || null
      badgeEl = opts.badgeEl || null
      playBtn = opts.playBtn || null
      seekEl = opts.seekEl || null
      timeEl = opts.timeEl || null
      loopChk = opts.loopChk || null
      frameScaleEl = opts.frameScaleEl || null
      if (opts.mirrorVideo) bindMirror(opts.mirrorVideo)
      destroyed = false
      currentState = buildState()
      initGl()
      wireControls()
      startPump()
    },
    setMirrorVideo(el) {
      bindMirror(el)
    },
    /** 载入媒体（file:// 或 blob: URL）；加载后停留首帧，由用户/宿主播放 */
    loadMedia(src) {
      stopPump()
      mediaVideo.src = src
      srcType = 'video'
      mediaLastTime = -1
      mediaVideo.onloadedmetadata = () => {
        try { mediaVideo.currentTime = 0 } catch (e) { /* ignore */ }
        updateTime()
        renderFrame()
      }
      startPump()
    },
    /** 内容区参数（预览台同名语义）：tf/gamut 为输入解读；dispPeak=目标渲染亮度（hdr_preview 显示峰值滑块）；
     *  outTf='pq'|'hlg' 时内容按所选输出传函（Eclipsa 导出编码）呈现（渲染数学不变——传函只是编码，
     *  输入解读仍由 tf 决定，故 SDR 源不会过曝/发暗，观感与导出文件一致）。 */
    applyContent(p) {
      if (p && TF_OPTIONS.indexOf(p.tf) >= 0) state.tf = p.tf
      if (p && GAMUT_ORDER.indexOf(p.gamut) >= 0) state.gamut = p.gamut
      if (p && typeof p.dispPeak === 'number' && isFinite(p.dispPeak)) state.dispPeak = p.dispPeak
      if (p && (p.outTf === 'pq' || p.outTf === 'hlg' || p.outTf === null || p.outTf === undefined)) state.outTf = p.outTf || null
      scheduleRender()
    },
    getContentParams() {
      return { tf: state.tf, gamut: state.gamut, maxNits: MAX_NITS[state.tf] || 203 }
    },
    /** 立即刷新当前帧（参数变化 / seek 后调用） */
    refreshFrame() {
      if (!mediaVideo.readyState) return
      mediaLastTime = mediaVideo.currentTime - 1
      renderFrame()
    },
    play() { const p = mediaVideo.play(); if (p && p.catch) p.catch(() => { /* ignore */ }) },
    pause() { try { mediaVideo.pause() } catch (e) { /* ignore */ } },
    seekTo(t) {
      if (!isFinite(t)) return
      mediaVideo.currentTime = Math.max(0, t)
      mediaLastTime = t - 1
    },
    isPaused() { return mediaVideo.paused },
    destroy() {
      destroyed = true
      stopPump()
      try { mediaVideo.pause() } catch (e) { /* ignore */ }
      if (mediaVideo.src) { try { mediaVideo.removeAttribute('src'); mediaVideo.load() } catch (e) { /* ignore */ } }
      mediaGl = null
      mediaGlFailed = false
    },
  }
})()