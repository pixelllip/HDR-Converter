/**
 * 临时验证：新后端（maxBoost 64）的 RGB 通道、HDR 强度 6 与 50% 预览
 */
const sharp = require('sharp')
const path = require('path')
const os = require('os')
const fs = require('fs')
const { ensureBackend, stopBackend, httpJson } = require('./backend_test_util')

async function main() {
    const tmp = path.join(os.tmpdir(), 'hdr_verify_advanced.png')
    // 含高光的 1200x800 测试图
    const { createCanvas } = {} // 不用 canvas，用 sharp 生成渐变
    const w = 1200, h = 800
    const rgba = Buffer.alloc(w * h * 4)
    for (let y = 0; y < h; y++) {
        for (let x = 0; x < w; x++) {
            const i = (y * w + x) * 4
            const bright = (x / w) * 255
            rgba[i] = bright
            rgba[i + 1] = Math.min(255, Math.round(bright * 0.8))
            rgba[i + 2] = Math.min(255, Math.round(bright * 0.6))
            rgba[i + 3] = 255
        }
    }
    await sharp(rgba, { raw: { width: w, height: h, channels: 4 } }).png().toFile(tmp)
    console.log('测试图:', tmp, w + 'x' + h)

    await ensureBackend()
    const backend = await httpJson('GET', '/backend')
    console.log('后端方式: method=' + backend.method, 'threads=' + backend.threads)

    // 1) /preview：png + RGB 通道调整，应成功且 50% 尺寸
    const pv = await httpJson('POST', '/preview', {
        inputPath: tmp,
        settings: { outputFormat: 'png', rgbAdjustment: { red: 0.5, green: 1.5, blue: 0.8 }, hdrIntensity: 1.18, gamma: 0.9 }
    })
    console.log('preview(png+RGB):', pv.width + 'x' + pv.height, '期望', Math.round(w * 0.5) + 'x' + Math.round(h * 0.5),
        pv.dataUrl ? 'dataUrl长度=' + pv.dataUrl.length : '无dataUrl')

    // 2) /convert：Ultra HDR + hdrIntensity=6
    const out = path.join(os.tmpdir(), 'hdr_verify_intensity6.jpg')
    try { fs.unlinkSync(out) } catch (e) { }
    const res = await httpJson('POST', '/convert', {
        inputPath: tmp,
        outputPath: out,
        settings: { outputFormat: 'jpg', hdrIntensity: 6.0, gamma: 0.9, quality: 0.9 }
    })
    console.log('convert(jpg,intensity=6):', JSON.stringify({ success: res.success, message: res.message, output: res.outputPath }))

    if (res.success && fs.existsSync(out)) {
        const size = fs.statSync(out).size
        console.log('输出文件大小:', size, '字节')
        // 结构校验（check_structure.js 校验 Ultra HDR 结构）
        const { execSync } = require('child_process')
        try {
            const chk = execSync('node ' + JSON.stringify(path.join(__dirname, 'check_structure.js')) + ' ' + JSON.stringify(out), { encoding: 'utf8' })
            console.log('结构校验输出:\n' + chk)
        } catch (e) {
            console.log('结构校验(stderr):', (e.stderr || e.message || '').toString().slice(0, 300))
        }
    }

    stopBackend()
    try { fs.unlinkSync(tmp) } catch (e) { }
    try { fs.unlinkSync(out) } catch (e) { }
    process.exit(0)
}

main().catch((e) => { console.error('验证失败:', e); stopBackend(); process.exit(1) })
