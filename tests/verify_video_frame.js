/**
 * 验证视频逐帧重建端点 /video-frame（2026-08-12；2026-09 更新：链路唯一模式为单层色调映射 transform）
 *
 * 素材：D:\video\output\img_0.jpg（SDR 帧）
 * 校验：
 *   1. 返回 16-bit PAM（P7 头、大端 RGB）
 *   2. 单层色调映射生效：yHdr = (yLin × 曝光)^γ，曝光=peak=8、γ=0.9 → 亮部显著高于 SDR 白（>1.0）
 *   3. 输出被钳制在 [0, peak]（不超峰值）
 *
 * 用法：node tests/verify_video_frame.js
 */
const fs = require('fs')
const path = require('path')
const { ensureBackend, stopBackend, httpJson } = require('./backend_test_util')

const SRC = 'D:\\video\\output\\img_0.jpg'
const GAMMA = 0.9
const PEAK = 8.0

async function main() {
    const port = await ensureBackend()
    console.log('后端端口:', port)

    // 用 /video-frame 重建第一帧（mode=transform：单层色调映射）
    const t0 = Date.now()
    const resp = await httpJson('POST', '/video-frame', {
        inputPath: SRC,
        settings: { gamma: GAMMA, outputFormat: 'jpg' },
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

    let maxHdr = 0
    let aboveWhite = 0 // yHdr > 1.0（SDR 白点以上）像素数
    let clamped = 0    // 达到峰值钳制上限的像素数
    for (let i = 0; i < n; i++) {
        const o = i * 6
        const r = (data.readUInt16BE(o)) / 65535 * PEAK
        const g = (data.readUInt16BE(o + 2)) / 65535 * PEAK
        const b = (data.readUInt16BE(o + 4)) / 65535 * PEAK
        const yHdr = 0.2126 * r + 0.7152 * g + 0.0722 * b
        if (yHdr > 1.0) aboveWhite++
        if (yHdr >= PEAK * 0.999) clamped++
        if (yHdr > maxHdr) maxHdr = yHdr
    }
    console.log(`最大 HDR 线性亮度: ${maxHdr.toFixed(3)}（SDR 白点=1.0；${(maxHdr * 100).toFixed(0)} 尼特等效）`)
    console.log(`超过 SDR 白点像素: ${aboveWhite}（单层色调映射整体提亮，亮部应显著 >1.0）`)
    console.log(`钳制到峰值像素: ${clamped}（不应大面积过曝钳制）`)

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