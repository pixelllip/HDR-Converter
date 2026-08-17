/**
 * 验证视频 RGB 通道参数生效（2026-08-12）
 * /video-frame mode=transform 与 gainmap 下，不同 rgbAdjustment 应改变输出通道均值
 */
const path = require('path')
const { ensureBackend, stopBackend, httpJson } = require('./backend_test_util')

const FRAME = 'c:/Users/Administrator/Documents/Java/hdr_electron/tests/tmp_video_hdr/frame_rgb.jpg'

// 生成一张中性灰帧（若不存在）
const fs = require('fs')
if (!fs.existsSync(FRAME)) {
    const { spawnSync } = require('child_process')
    spawnSync('c:/Users/Administrator/Documents/Java/hdr_electron/backend/ffmpeg/ffmpeg.exe',
        ['-y', '-f', 'lavfi', '-i', 'color=size=640x360:rate=1:color=0x808080', '-frames:v', '1', FRAME],
        { windowsHide: true })
}

function analyze(pamB64) {
    const data = Buffer.from(pamB64, 'base64')
    const end = data.indexOf(Buffer.from('ENDHDR\n')) + 7
    const px = data.slice(end)
    const w = 640, h = 360, n = w * h
    let rs = 0, gs = 0, bs = 0
    for (let i = 0; i < n; i++) {
        const o = i * 6
        rs += data.readUInt16BE(end + o)
        gs += data.readUInt16BE(end + o + 2)
        bs += data.readUInt16BE(end + o + 4)
    }
    return { r: rs / n, g: gs / n, b: bs / n }
}

async function main() {
    const port = await ensureBackend()
    const base = { hdrIntensity: 1.5, fineTuneBrightness: 1.0, gamma: 0.9, outputFormat: 'jpg' }

    for (const mode of ['transform', 'gainmap']) {
        console.log('\n=== mode=' + mode + ' ===')
        const neutral = await httpJson('POST', '/video-frame', {
            inputPath: FRAME, settings: { ...base, rgbAdjustment: { red: 1, green: 1, blue: 1 } }, peak: 8, mode
        })
        const boosted = await httpJson('POST', '/video-frame', {
            inputPath: FRAME, settings: { ...base, rgbAdjustment: { red: 2, green: 0.5, blue: 1 } }, peak: 8, mode
        })
        const a = analyze(neutral.pamBase64)
        const b = analyze(boosted.pamBase64)
        console.log('  neutral R/G/B:', a.r.toFixed(0), a.g.toFixed(0), a.b.toFixed(0))
        console.log('  red=2.0,g=0.5,b=1.0 R/G/B:', b.r.toFixed(0), b.g.toFixed(0), b.b.toFixed(0))
        if (mode === 'transform') {
            const ok = b.r > a.r * 1.5 && b.g < a.g * 0.7
            console.log((ok ? '✅' : '❌') + ' transform（直接转·ICC 增益式）RGB 生效（红↑、绿↓）')
            if (!ok) process.exitCode = 1
        } else {
            const ok = Math.abs(b.r - a.r) < a.r * 0.02 && Math.abs(b.g - a.g) < a.g * 0.02
            console.log((ok ? '✅' : '❌') + ' gainmap（Ultra HDR 式）忽略 RGB（红/绿不变）')
            if (!ok) process.exitCode = 1
        }
    }
    stopBackend()
    console.log('\n完成')
}
main().catch((e) => { console.error('❌', e.message); stopBackend(); process.exit(1) })
