/**
 * 验证视频首帧 HDR 预览链路（2026-08-12）
 *
 * 流程（前端 HDR 区首帧预览所走的路径）：
 *   1. extractFirstFrame(sdr_test.mp4) → 首帧临时文件 + dataUrl
 *   2. 后端 /preview（图片 HDR 链路）：jpg_icc（direct 模式对应）与 jpg（Ultra HDR，frames 模式对应）
 *   3. 校验返回 HDR dataUrl
 *
 * 用法：node tests/verify_video_firstframe_preview.js
 */
const fs = require('fs')
const path = require('path')
const vc = require('../video_converter')
const { ensureBackend, stopBackend, httpJson } = require('./backend_test_util')

const TMP = path.join(__dirname, 'tmp_video_hdr')
const SDR = path.join(TMP, 'sdr_test.mp4')

async function main() {
    if (!fs.existsSync(SDR)) {
        console.log('未找到 sdr_test.mp4，请先运行 node tests/verify_video_convert.js')
        process.exit(1)
    }
    console.log('\n========== 1) 提取首帧 ==========')
    const ff = await vc.extractFirstFrame(SDR)
    console.log('✅ dataUrl 长度:', ff.dataUrl.length, 'bytes; 帧文件:', ff.framePath)
    console.log('✅ 帧文件存在:', fs.existsSync(ff.framePath))

    const port = await ensureBackend()
    console.log('\n========== 2) 图片 HDR 链路 /preview ==========')
    for (const fmt of ['jpg_icc', 'jpg']) {
        const r = await httpJson('POST', '/preview', {
            inputPath: ff.framePath,
            settings: { hdrIntensity: 1.5, fineTuneBrightness: 1.0, gamma: 0.9, outputFormat: fmt, quality: 0.9 }
        })
        const ok = !!r.dataUrl && r.dataUrl.startsWith('data:image/')
        console.log((ok ? '✅' : '❌') + ` ${fmt} 首帧 HDR 预览: dataUrl ${r.dataUrl ? r.dataUrl.length : 0} bytes, ${r.width}x${r.height}`)
    }

    stopBackend()
    console.log('\n首帧 HDR 预览链路完成。')
}

main().catch((e) => {
    console.error('❌ 验证失败:', e.message)
    stopBackend()
    process.exit(1)
})
