/**
 * 临时验证脚本：确认 /preview 返回原图 50% 分辨率
 */
const sharp = require('sharp')
const path = require('path')
const os = require('os')
const fs = require('fs')
const { ensureBackend, stopBackend, httpJson } = require('./backend_test_util')

async function main() {
    // 1. 生成 2000x1500 测试图
    const W = 2000, H = 1500
    const tmp = path.join(os.tmpdir(), 'hdr_preview_test_2000x1500.png')
    await sharp({ create: { width: W, height: H, channels: 3, background: { r: 120, g: 160, b: 220 } } }).png().toFile(tmp)
    console.log('生成测试图:', tmp, W + 'x' + H)

    // 2. 启动后端
    await ensureBackend()
    console.log('后端已就绪')

    // 3. 调用 /preview
    const res = await httpJson('POST', '/preview', {
        inputPath: tmp,
        settings: { outputFormat: 'jpg' }
    })
    console.log('preview 返回 width=', res.width, 'height=', res.height, 'aspectRatio=', res.aspectRatio.toFixed(4))

    const expectW = Math.round(W * 0.5)
    const expectH = Math.round(H * 0.5)
    const ok = res.width === expectW && res.height === expectH
    console.log('期望(50%):', expectW + 'x' + expectH, ok ? '✅ 通过' : '❌ 不匹配')

    // 4. 清理
    fs.unlinkSync(tmp)
    stopBackend()
    process.exit(ok ? 0 : 1)
}

main().catch((e) => {
    console.error('验证失败:', e)
    stopBackend()
    process.exit(1)
})
