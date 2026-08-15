/**
 * 验证视频两条链路使用 GPU NVENC 编码（2026-08-12）
 *
 * 素材：tests/tmp_video_hdr/sdr_test.mp4（由 verify_video_convert.js 生成，若不存在则提示先跑）
 * 流程：
 *   1. 链路 1（直接滤镜）+ hevc_nvenc → HDR10
 *   2. 链路 2（逐帧增益图）+ hevc_nvenc → HDR10
 *   3. checkHdrMetadata 校验元数据全绿（容器盒 / SEI / 色彩属性）
 *
 * 用法：node tests/verify_video_nvenc.js
 */
const fs = require('fs')
const path = require('path')
const vc = require('../video_converter')
const { ensureBackend, stopBackend } = require('./backend_test_util')
const { checkHdrMetadata } = require('./verify_hdr_metadata')

const TMP = path.join(__dirname, 'tmp_video_hdr')
const SDR = path.join(TMP, 'sdr_test.mp4')

async function main() {
    if (!fs.existsSync(SDR)) {
        console.log('未找到 sdr_test.mp4，请先运行 node tests/verify_video_convert.js 生成测试素材')
        process.exit(1)
    }

    // 首帧提取（源视频首帧预览）
    console.log('\n========== 首帧提取 ==========')
    const ff = await vc.extractFirstFrame(SDR)
    console.log('✅ 首帧 dataUrl 长度:', ff.dataUrl ? ff.dataUrl.length : 0, 'bytes(base64)')

    // 链路 1 + 默认（不传 encoder → GPU NVENC）
    console.log('\n========== 链路 1（直接转 ICC 增益式）+ 默认 GPU NVENC ==========')
    const port = await ensureBackend()
    const out1 = path.join(TMP, 'video_l1_nvenc.mp4')
    const t1 = Date.now()
    const r1 = await vc.convertVideoDirect(SDR, out1, { hdrIntensity: 2.0, fineTuneBrightness: 1.0, gamma: 0.9, crf: 20 },
        { backendPort: port }, () => { })
    console.log(`✅ 输出 ${((Date.now() - t1) / 1000).toFixed(1)}s, 实际编码器: ${r1.encoder}`)
    await checkHdrMetadata(out1)

    // 链路 2 + 默认（不传 encoder → GPU NVENC）
    console.log('\n========== 链路 2（逐帧增益图）+ 默认 GPU NVENC ==========')
    const out2 = path.join(TMP, 'video_l2_nvenc.mp4')
    const t2 = Date.now()
    const r2 = await vc.convertVideoFrames(SDR, out2, { hdrIntensity: 2.4, gamma: 0.9, crf: 20, maxWidth: 640 },
        { backendPort: port }, () => { })
    console.log(`✅ 输出 ${((Date.now() - t2) / 1000).toFixed(1)}s, 实际编码器: ${r2.encoder}`)
    await checkHdrMetadata(out2)

    stopBackend()
    console.log('\n默认 GPU NVENC 两条链路全部完成。')
}

main().catch((e) => {
    console.error('\n❌ 验证失败:', e.message)
    stopBackend()
    process.exit(1)
})
