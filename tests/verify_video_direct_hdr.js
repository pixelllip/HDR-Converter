/**
 * 验证「直接转」（/video-frame mode=transform）输出真正 HDR（2026-08-13 回归）
 *
 * 背景 bug：直接滤镜模式输出不是 HDR。
 * 根因：后端曝光 = hdrIntensity(EV)×fineTuneBrightness，UI 默认 EV≈1.5×0.35=0.525
 *       → SDR 白（线性 1.0）只映射到 ~114 尼特，被压暗成非 HDR。
 * 修复：①后端曝光 = peak(=峰值/白点=2^EV)；②微调明暗滑块已移除，直接转不再乘
 *       fineTuneBrightness（乘 <1 会把 HDR 压暗）。
 *
 * 断言：纯白帧在 UI 默认参数（白点203/峰值574）下，SDR 白应映射到 >1.5×白点 尼特；
 *       且即使传入 fineTuneBrightness=0.3（旧默认）也不受影响（后端已忽略）。
 */
const path = require('path')
const fs = require('fs')
const { spawnSync } = require('child_process')
const { ensureBackend, stopBackend, httpJson } = require('./backend_test_util')

const FRAME = path.join(__dirname, 'tmp_video_hdr', 'dbg_white.png')
const FFMPEG = path.join(__dirname, '..', 'backend', 'ffmpeg', 'ffmpeg.exe')

if (!fs.existsSync(FRAME)) {
    spawnSync(FFMPEG,
        ['-y', '-f', 'lavfi', '-i', 'color=size=640x360:rate=1:color=0xffffff', '-frames:v', '1', FRAME],
        { windowsHide: true })
}

/** 解析 PAM：返回最大归一化线性值（文件 1.0 = peak = peakNits 尼特） */
function maxPam(pamB64) {
    const data = Buffer.from(pamB64, 'base64')
    const end = data.indexOf(Buffer.from('ENDHDR\n')) + 7
    const n = ((data.length - end) / 6) | 0
    let max = 0
    for (let i = 0; i < n; i++) {
        const o = end + i * 6
        const v = Math.max(data.readUInt16BE(o), data.readUInt16BE(o + 2), data.readUInt16BE(o + 4)) / 65535
        if (v > max) max = v
    }
    return max
}

async function main() {
    const port = await ensureBackend()
    const whiteNits = 203
    const peakNits = 574
    const peak = peakNits / whiteNits // ≈2.83 = 2^EV
    const npl = peakNits

    const resp = await httpJson('POST', '/video-frame', {
        inputPath: FRAME,
        settings: {
            hdrIntensity: peak, // direct 模式前端传 2^EV（=峰值/白点）；后端曝光=peak
            fineTuneBrightness: 0.3, // 旧默认值：后端已忽略（微调明暗移除），不应压暗
            gamma: 0.9,
            rgbAdjustment: { red: 1, green: 1, blue: 1 },
            outputFormat: 'jpg'
        },
        peak,
        mode: 'transform'
    })
    const m = maxPam(resp.pamBase64)
    const nitsOut = m * npl
    const threshold = whiteNits * 1.5
    console.log(`SDR 白（线性1.0）映射 ${nitsOut.toFixed(0)} 尼特（白点 ${whiteNits}，峰值 ${peakNits}，阈值 ${threshold.toFixed(0)}）`)
    if (nitsOut >= threshold) {
        console.log('✅ 直接转输出为真正 HDR（SDR 白明显超过白点）')
    } else {
        console.log('❌ 直接转输出非 HDR（SDR 白未超过白点，可能被曝光压暗）')
        process.exitCode = 1
    }
    stopBackend()
    console.log('\n完成')
}
main().catch((e) => { console.error('❌', e.message); stopBackend(); process.exit(1) })
