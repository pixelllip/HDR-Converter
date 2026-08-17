/**
 * 验证 HDR10 输出是否完整声明了 Chromium 可识别的 HDR 元数据（2026-08-12）
 *
 * 检查项（Chromium 判定 HDR 视频的依据）：
 *   1. 流级：profile=Main 10 + tag=hvc1
 *   2. 流级：color_primaries=bt2020 / transfer=smpte2084 / space=bt2020nc / range=tv
 *   3. 容器：colr / mdcv / clli 三个盒存在（Chromium MP4 demuxer 读取）
 *   4. 流 side data：Mastering display metadata（P3 主色 + 1000 尼特）+ Content light level
 *   5. 码流 SEI：帧级 Mastering Display / Content Light Level（x265 写入）
 *
 * 用法：node tests/verify_hdr_metadata.js [视频路径]
 *   默认检查 tests/tmp_video_hdr/meta_injected.mp4
 */
const { spawn } = require('child_process')
const path = require('path')

const FFPROBE = path.join(__dirname, '..', 'backend', 'ffmpeg', 'ffprobe.exe')
const FFMPEG = path.join(__dirname, '..', 'backend', 'ffmpeg', 'ffmpeg.exe')
const DEFAULT_FILE = path.join(__dirname, 'tmp_video_hdr', 'meta_injected.mp4')

function run(cmd, args) {
    return new Promise((resolve, reject) => {
        const p = spawn(cmd, args, { windowsHide: true })
        let out = ''
        let err = ''
        p.stdout.on('data', (d) => (out += d))
        p.stderr.on('data', (d) => (err += d))
        p.on('error', reject)
        p.on('close', (code) => (code === 0 ? resolve({ out, err }) : reject(new Error(err.slice(-500)))))
    })
}

// ffprobe 有理数 "34000/50000" → 0.68
function rat(v) {
    const p = String(v).split('/')
    return parseInt(p[0], 10) / parseInt(p[1], 10)
}

async function checkHdrMetadata(filePath) {
    const results = []
    const ok = (name, pass, detail) => {
        results.push({ name, pass, detail })
        console.log((pass ? '✅' : '❌') + ' ' + name + (detail ? '  (' + detail + ')' : ''))
    }

    // 1-2) 流级元数据
    const streamOut = await run(FFPROBE, ['-v', 'error', '-select_streams', 'v:0', '-show_streams',
        '-show_entries', 'stream=profile,codec_tag_string,pix_fmt,color_primaries,color_transfer,color_space,color_range,side_data_list',
        '-of', 'json', filePath])
    const stream = JSON.parse(streamOut.out).streams[0]
    ok('profile=Main 10', stream.profile === 'Main 10', stream.profile)
    ok('tag=hvc1', stream.codec_tag_string === 'hvc1', stream.codec_tag_string)
    ok('pix_fmt=yuv420p10le', stream.pix_fmt === 'yuv420p10le', stream.pix_fmt)
    ok('primaries=bt2020', stream.color_primaries === 'bt2020', stream.color_primaries)
    ok('transfer=smpte2084', stream.color_transfer === 'smpte2084', stream.color_transfer)
    ok('space=bt2020nc', stream.color_space === 'bt2020nc', stream.color_space)
    ok('range=tv', stream.color_range === 'tv', stream.color_range)

    // 3) 容器盒 colr / mdcv / clli（ffmpeg -v trace 输出在 stderr）
    const traceRun = await run(FFMPEG, ['-v', 'trace', '-i', filePath, '-f', 'null', '-'])
    const trace = traceRun.out + traceRun.err
    for (const box of ['colr', 'mdcv', 'clli']) {
        ok(`容器盒 ${box}`, trace.includes(`type:'${box}'`), '')
    }

    // 4) 流 side data（来自 mdcv/clli 盒 → Chromium demuxer）
    const sdList = (stream.side_data_list || []).map((d) => d.side_data_type)
    const md = sdList.includes('Mastering display metadata')
    const cl = sdList.includes('Content light level metadata')
    ok('流 side data: Mastering display', md, '')
    ok('流 side data: Content light level', cl, '')
    if (md) {
        const m = stream.side_data_list.find((d) => d.side_data_type === 'Mastering display metadata')
        console.log('    mastering display 主色 R=(' + rat(m.red_x).toFixed(3) + ',' + rat(m.red_y).toFixed(3) + ') G=(' +
            rat(m.green_x).toFixed(3) + ',' + rat(m.green_y).toFixed(3) + ') B=(' +
            rat(m.blue_x).toFixed(3) + ',' + rat(m.blue_y).toFixed(3) + ') 峰值=' +
            rat(m.max_luminance).toFixed(1) + ' 尼特')
    }
    if (cl) {
        const c = stream.side_data_list.find((d) => d.side_data_type === 'Content light level metadata')
        console.log('    MaxCLL=' + c.max_content + '  MaxFALL=' + c.max_average)
    }

    // 5) 码流 SEI（帧级）
    const frameOut = await run(FFPROBE, ['-v', 'error', '-select_streams', 'v:0', '-show_frames',
        '-show_entries', 'frame=side_data_list', '-of', 'json', filePath])
    const frames = JSON.parse(frameOut.out).frames || []
    const frameTypes = (frames[0] && (frames[0].side_data_list || []).map((d) => d.side_data_type)) || []
    ok('码流 SEI: Mastering Display', frameTypes.includes('Mastering display metadata'), '')
    ok('码流 SEI: Content light level', frameTypes.includes('Content light level metadata'), '')

    const allOk = results.every((r) => r.pass)
    console.log(allOk ? '\n✅ 全部通过：Chromium 可识别为 HDR 视频' : '\n❌ 存在未通过的检查项')
    return allOk
}

module.exports = { checkHdrMetadata }

if (require.main === module) {
    checkHdrMetadata(process.argv[2] || DEFAULT_FILE)
        .then((ok) => process.exit(ok ? 0 : 1))
        .catch((e) => {
            console.error('❌ 验证失败:', e.message)
            process.exit(1)
        })
}
