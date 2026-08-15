/**
 * 端到端验证视频两条链路转换（2026-08-12）
 *
 * 素材：D:\video\output\img_0..29.jpg（2560x1440 SDR 帧）
 * 流程：
 *   1. 用 ffmpeg 从 30 帧合成一段 SDR 测试视频（缩到 640x360，快速）
 *   2. 链路 1（直接滤镜）：SDR 视频 → HDR10 MP4（单条滤镜链）
 *   3. 链路 2（逐帧增益图）：SDR 视频 → 拆帧 → /video-frame 重建 → HDR10 MP4
 *   4. 验证：ffprobe 色彩元数据（bt2020/smpte2084/yuv420p10le）+ 解码首帧峰值线性亮度
 *
 * 用法：node tests/verify_video_convert.js
 */
const fs = require('fs')
const path = require('path')
const { spawn } = require('child_process')
const vc = require('../video_converter')
const { ensureBackend, stopBackend } = require('./backend_test_util')
const { checkHdrMetadata } = require('./verify_hdr_metadata')

const FFMPEG = path.join(__dirname, '..', 'backend', 'ffmpeg', 'ffmpeg.exe')
const FFPROBE = path.join(__dirname, '..', 'backend', 'ffmpeg', 'ffprobe.exe')
const SRC_DIR = 'D:\\video\\output'
const TMP = path.join(__dirname, 'tmp_video_hdr')

function run(cmd, args) {
    return new Promise((resolve, reject) => {
        const p = spawn(cmd, args, { windowsHide: true })
        let err = ''
        p.stderr.on('data', (d) => (err += d))
        p.on('error', reject)
        p.on('close', (code) =>
            code === 0 ? resolve() : reject(new Error(`exit=${code}\n${err.slice(-600)}`)))
    })
}

async function probeMeta(p) {
    const out = await new Promise((resolve, reject) => {
        const c = spawn(FFPROBE, ['-v', 'error', '-select_streams', 'v:0', '-show_streams', '-show_frames',
            '-show_entries', 'stream=pix_fmt,color_primaries,color_transfer,color_space,width,height:frame=side_data_list',
            '-of', 'json', p], { windowsHide: true })
        let o = ''
        let e = ''
        c.stdout.on('data', (d) => (o += d))
        c.stderr.on('data', (d) => (e += d))
        c.on('close', (code) => code === 0 ? resolve(o) : reject(new Error(e.slice(-400))))
    })
    const j = JSON.parse(out)
    const s = (j.streams && j.streams[0]) || {}
    return { pix_fmt: s.pix_fmt, cp: s.color_primaries, ct: s.color_transfer, cs: s.color_space, w: s.width, h: s.height }
}

/** 解码首帧测峰值线性亮度（gbrpf32le 平面格式） */
async function peakLum(p) {
    const out = await new Promise((resolve, reject) => {
        const c = spawn(FFMPEG, ['-v', 'error', '-i', p, '-frames:v', '1',
            '-vf', 'zscale=tin=smpte2084:t=linear:npl=100:rin=limited:r=full,format=gbrpf32le',
            '-f', 'rawvideo', '-pix_fmt', 'gbrpf32le', '-'], { windowsHide: true })
        const chunks = []
        let e = ''
        c.stdout.on('data', (d) => chunks.push(d))
        c.stderr.on('data', (d) => (e += d))
        c.on('close', (code) => code === 0 ? resolve(Buffer.concat(chunks)) : reject(new Error(e.slice(-400))))
    })
    const W = 640, H = 360
    let maxY = 0
    // gbrpf32le 平面：先 G 再 B 再 R
    for (let i = 0; i < W * H; i++) {
        const g = out.readFloatLE(i * 4)
        const b = out.readFloatLE(W * H * 4 + i * 4)
        const r = out.readFloatLE(2 * W * H * 4 + i * 4)
        const y = 0.2126 * r + 0.7152 * g + 0.0722 * b
        if (y > maxY) maxY = y
    }
    return maxY
}

async function main() {
    fs.mkdirSync(TMP, { recursive: true })
    const sdrTest = path.join(TMP, 'sdr_test.mp4')

    // 1) 合成 SDR 测试视频（30 帧，缩到 640x360）
    console.log('\n========== 合成 SDR 测试视频 ==========')
    await run(FFMPEG, ['-y', '-start_number', '0', '-framerate', '10', '-i', path.join(SRC_DIR, 'img_%d.jpg'),
        '-frames:v', '30', '-vf', 'scale=640:360', '-c:v', 'libx264', '-preset', 'veryfast', '-crf', '18', sdrTest])
    console.log('✅ SDR 测试视频:', sdrTest)

    // 2) 链路 1：直接转（单层色调映射，图片 ICC 增益式）
    console.log('\n========== 链路 1：直接转（图片 ICC 增益式） ==========')
    const port = await ensureBackend()
    const out1 = path.join(TMP, 'video_l1_hdr10.mp4')
    const t1 = Date.now()
    await vc.convertVideoDirect(sdrTest, out1, { hdrIntensity: 2.0, fineTuneBrightness: 1.0, gamma: 0.9, crf: 20 },
        { backendPort: port }, (v, m) => process.stdout.write(`\r  ${Math.round(v * 100)}%`))
    console.log('\n✅ 链路 1 输出:', out1, `(${((Date.now() - t1) / 1000).toFixed(1)}s)`)
    console.log(await probeMeta(out1))
    console.log('  首帧峰值线性亮度:', (await peakLum(out1)).toFixed(3))
    console.log('  --- HDR 元数据检查 ---')
    await checkHdrMetadata(out1)
    // 3) 链路 2：逐帧增益图
    console.log('\n========== 链路 2：逐帧增益图 ==========')
    const out2 = path.join(TMP, 'video_l2_hdr10.mp4')
    const t2 = Date.now()
    await vc.convertVideoFrames(sdrTest, out2, { hdrIntensity: 2.4, gamma: 0.9, crf: 20, maxWidth: 640 },
        { backendPort: port }, (v, m) => process.stdout.write(`\r  ${Math.round(v * 100)}% ${m}`))
    console.log('\n✅ 链路 2 输出:', out2, `(${((Date.now() - t2) / 1000).toFixed(1)}s)`)
    console.log(await probeMeta(out2))
    console.log('  首帧峰值线性亮度:', (await peakLum(out2)).toFixed(3))
    console.log('  --- HDR 元数据检查 ---')
    await checkHdrMetadata(out2)

    stopBackend()
    console.log('\n全部完成。')
}

main().catch((e) => {
    console.error('\n❌ 验证失败:', e.message)
    stopBackend()
    process.exit(1)
})
