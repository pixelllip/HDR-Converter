'use strict'
/** 跑产品模块 attachSt2094_50（第三格式生产链路） */
const path = require('path')
const { attachSt2094_50 } = require(path.resolve(__dirname, '..', '..', '..', 'st2094_50_inject.js'))

const ROOT = path.resolve(__dirname, '..', '..', '..')
const INPUT = 'D:/video/video_sdr/bg_waifu2x_2x_2n_mp4_HDR10_frames.mp4'
const OUTPUT = 'D:/video/video_sdr/bg_waifu2x_2x_2n_mp4_Eclipsa_prod3rd.mp4'
const SCHEME = process.argv[2] || 'scene' // scene | uniform

attachSt2094_50(INPUT, OUTPUT, {
  ffmpeg: path.join(ROOT, 'backend', 'ffmpeg', 'ffmpeg.exe'),
  ffprobe: path.join(ROOT, 'backend', 'ffmpeg', 'ffprobe.exe'),
  maxCll: 574, maxFall: 400,
  windowScheme: SCHEME,
  onProgress: (v, m) => { if (m) console.log('  ' + m) }
}).then(r => {
  console.log('RESULT ' + JSON.stringify(r, null, 1))
  process.exit(0)
}).catch(e => { console.error('FAIL', e); process.exit(1) })
