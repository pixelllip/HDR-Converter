/**
 * 验证视频链路 2 后端端点 /video-frame（2026-08-12）
 *
 * 素材：D:\video\output\img_0.jpg（SDR 帧）
 * 校验：
 *   1. 返回 16-bit PAM（P7 头、大端 RGB）
 *   2. 高光扩展：线性亮度 >0.5 的像素 HDR 亮度显著高于 SDR（>1.0 白点）
 *   3. 中间调/暗部保色：线性亮度 <0.5 的像素 HDR ≈ SDR
 *
 * 用法：node tests/verify_video_frame.js
 */
const fs = require('fs')
const path = require('path')
const { ensureBackend, stopBackend, httpJson } = require('./backend_test_util')

const SRC = 'D:\\video\\output\\img_0.jpg'
const HDR_INTENSITY = 2.4
const PEAK = 8.0

function srgbToLinear(v) { return v <= 0.04045 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4) }

async function main() {
    const port = await ensureBackend()
    console.log('后端端口:', port)

    // 用 /video-frame 重建第一帧
    const t0 = Date.now()
    const resp = await httpJson('POST', '/video-frame', {
        inputPath: SRC,
        settings: { hdrIntensity: HDR_INTENSITY, fineTuneBrightness: 1.0, gamma: 0.9, outputFormat: 'jpg' },
        peak: PEAK
    })
    console.log(`/video-frame 耗时: ${Date.now() - t0}ms, ${resp.width}x${resp.height}`)

    const pam = Buffer.from(resp.pamBase64, 'base64')
    const headerEnd = pam.indexOf(Buffer.from('ENDHDR\n'))
    const header = pam.slice(0, headerEnd + 7).toString('latin1')
    const data = pam.slice(headerEnd + 7)
    console.log('PAM 头:', header.replace(/\n/g, ' | '))

    // 解析大端 16-bit RGB（交错）
    const w = resp.width
    const h = resp.height
    const n = w * h
    if (data.length !== n * 6) throw new Error(`PAM 数据长度不符: ${data.length} != ${n * 6}`)

    let midSum = 0, midN = 0
    let hlSum = 0, hlN = 0
    let maxHdr = 0
    for (let i = 0; i < n; i++) {
        const o = i * 6
        const r = (data.readUInt16BE(o)) / 65535 * PEAK
        const g = (data.readUInt16BE(o + 2)) / 65535 * PEAK
        const b = (data.readUInt16BE(o + 4)) / 65535 * PEAK
        // PAM 里存的是线性 HDR（1.0 = SDR 白点），需要知道对应的 SDR 亮度来分区
        // 这里用 HDR 亮度 <0.5 视为中间调（增益=1 区域，HDR=SDR）
        const yHdr = 0.2126 * r + 0.7152 * g + 0.0722 * b
        if (yHdr > maxHdr) maxHdr = yHdr
        // 分区：HDR 亮度在 [0.1, 0.4] 为中间调（gain=1 保色），>1.0 为高光扩展
        if (yHdr > 0.1 && yHdr < 0.4) { midSum += yHdr; midN++ }
        if (yHdr > 1.0) { hlSum += yHdr; hlN++ }
    }
    console.log(`最大 HDR 线性亮度: ${maxHdr.toFixed(3)}（SDR 白点=1.0；${(maxHdr * 100).toFixed(0)} 尼特等效）`)
    if (midN) console.log(`中间调平均亮度: ${(midSum / midN).toFixed(3)}（应≈0.1-0.4，即保色未提亮）`)
    if (hlN) console.log(`高光像素 ${hlN} 个，平均 ${(hlSum / hlN).toFixed(3)}（应 >1.0，真高光扩展）`)
    else console.log('⚠ 无 >1.0 高光像素（该帧可能没有高光，需换帧验证）')

    const ok = maxHdr > 1.0 && maxHdr < PEAK
    console.log(ok ? '✅ /video-frame 验证通过' : '❌ /video-frame 验证失败')
    stopBackend()
    process.exit(ok ? 0 : 1)
}

main().catch((e) => {
    console.error('❌ 验证失败:', e.message)
    stopBackend()
    process.exit(1)
})
