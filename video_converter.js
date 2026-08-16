/**
 * 视频转换模块（ffmpeg 封装）—— 主进程专用
 *
 * 链路 1（快速·直接滤镜）：SDR 视频 → ffmpeg 滤镜链（线性化 + npl 提亮）→ HDR10 MP4
 * 链路 2（精确·逐帧增益图）：SDR 视频 → 解码逐帧 → Kotlin 后端 /video-frame 重建线性 HDR
 *                          → 16-bit PAM → ffmpeg 编码 → HDR10 MP4
 *
 * 预览：转换后生成色调映射回 SDR 的预览 MP4（`<video>` 可播放、可拖动进度）；
 *       拖动进度时按需提取指定时间帧（-ss）→ tone map → JPEG data URL。
 */
const path = require('path')
const fs = require('fs')
const os = require('os')
const http = require('http')
const { spawn } = require('child_process')
const { injectHdrBoxes } = require('./mp4_hdr')

// 打包后：ffmpeg 在 resources/app.asar.unpacked（asar 里即使有 stub/副本 existsSync 也为 true，但不能 spawn）；
// 开发时在 __dirname 下。用 app.isPackaged 确定性选择；node 测试环境 require('electron') 不可用则视为开发路径。
let IS_PACKAGED = false
try {
  const { app } = require('electron')
  IS_PACKAGED = !!(app && app.isPackaged)
} catch (e) { /* 非 electron 环境（node 测试脚本） */ }
// spawn 的 cwd 必须是真实目录：打包后 __dirname 是 app.asar（文件不是目录），作 cwd 会 ENOENT
const RUN_CWD = IS_PACKAGED ? process.resourcesPath : __dirname
function resourcePath(rel) {
  if (IS_PACKAGED) return path.join(process.resourcesPath, 'app.asar.unpacked', rel)
  return path.join(__dirname, rel)
}

const FFMPEG = resourcePath(path.join('backend', 'ffmpeg', 'ffmpeg.exe'))
const FFPROBE = resourcePath(path.join('backend', 'ffmpeg', 'ffprobe.exe'))

// 链路 2 编码参数（与 tests/gen_uhdr_chain_video.js 一致）
// 默认白点 / 峰值亮度（用户可在界面调整）
const DEFAULT_WHITE_NITS = 203   // SDR 参考白（BT.2408）
const DEFAULT_PEAK_NITS = 1000   // 峰值亮度（高光上限 / max-cll）
const MASTER_DISPLAY = 'master-display=G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)L(10000000,1)'

// 逐帧重建并发 = 核心数（上限 8）。链路 2 的 /video-frame 不受后端信号量限制，这里自限并发。
// 后端每帧内部已是单线程（并行度由帧级并发提供），因此 并发×1 ≈ 核心数，恰好吃满 CPU：
// 8 核 → 8 并发（8 线程），无超订、无每帧线程创建/join 开销，吞吐最高。
// 内存：每帧（4K）约 33MB RGBA + 50MB PAM，8 并发峰值 ~700MB；4K 全分辨率且内存紧张时可调低。
const FRAME_CONCURRENCY = Math.max(1, Math.min(8, os.cpus().length))

// 正在运行的 ffmpeg 进程（用于取消）
const activeFFmpeg = new Set()

/** 取消所有运行中的 ffmpeg（taskkill /T 杀掉子进程） */
function cancelAllFFmpeg() {
    for (const proc of activeFFmpeg) {
        try {
            spawn('taskkill', ['/pid', String(proc.pid), '/T', '/F'], { windowsHide: true })
        } catch (e) { /* ignore */ }
        try { proc.kill() } catch (e) { /* ignore */ }
    }
    activeFFmpeg.clear()
}

/** 运行 ffmpeg 并解析 -progress pipe:1 进度（durationUs 已知时上报 0..1） */
function runFFmpeg(args, { onProgress, durationUs, cwd } = {}) {
    return new Promise((resolve, reject) => {
    const proc = spawn(FFMPEG, args, { windowsHide: true, cwd: cwd || RUN_CWD })
        activeFFmpeg.add(proc)
        let stderr = ''
        let lastProgress = 0
        proc.stderr.on('data', (d) => { stderr += d.toString() })
        proc.stdout.on('data', (d) => {
            const text = d.toString()
            const m = text.match(/out_time_us=(\d+)/)
            if (m && onProgress && durationUs) {
                const p = Math.min(1, parseInt(m[1], 10) / durationUs)
                if (p - lastProgress >= 0.001 || p >= 1) {
                    lastProgress = p
                    onProgress(p, '编码中…')
                }
            }
        })
        proc.on('error', (err) => {
            activeFFmpeg.delete(proc)
            reject(err)
        })
        proc.on('close', (code) => {
            activeFFmpeg.delete(proc)
            if (code === 0) resolve()
            else reject(new Error('ffmpeg 退出码 ' + code + '\n' + stderr.slice(-800)))
        })
    })
}

/** 运行 ffprobe 返回视频信息 JSON */
function ffprobeJson(args) {
    return new Promise((resolve, reject) => {
        const proc = spawn(FFPROBE, ['-v', 'error', '-of', 'json'].concat(args), { windowsHide: true })
        let out = ''
        let err = ''
        proc.stdout.on('data', (d) => (out += d))
        proc.stderr.on('data', (d) => (err += d))
        proc.on('error', reject)
        proc.on('close', (code) => {
            if (code !== 0) reject(new Error('ffprobe 失败: ' + err.slice(-400)))
            else {
                try { resolve(JSON.parse(out)) } catch (e) { reject(new Error('ffprobe 输出解析失败')) }
            }
        })
    })
}

/** 探测视频：时长 / 帧率 / 分辨率 / 音频 / 帧数 */
async function probeVideo(inputPath) {
    const j = await ffprobeJson(['-show_streams', '-show_format', inputPath])
    const vs = (j.streams || []).find((s) => s.codec_type === 'video')
    const as = (j.streams || []).find((s) => s.codec_type === 'audio')
    if (!vs) throw new Error('输入文件中没有视频流')
    const dur = parseFloat(vs.duration || j.format && j.format.duration || 0)
    const fps = evalFps(vs.avg_frame_rate || vs.r_frame_rate)
    const frames = Math.round(dur * fps)
    return {
        width: vs.width,
        height: vs.height,
        duration: dur,
        fps,
        frames,
        codec: vs.codec_name,
        pixFmt: vs.pix_fmt,
        hasAudio: !!as
    }
}

/** 解析 "30000/1001" 形式的帧率 */
function evalFps(rate) {
    const m = String(rate).split('/')
    if (m.length === 2) {
        const a = parseFloat(m[0]); const b = parseFloat(m[1])
        if (b) return a / b
    }
    return parseFloat(rate) || 30
}

/** 主进程内 HTTP JSON 请求（调用 Kotlin 后端 /video-frame 等） */
function httpJson(port, method, route, body) {
    return new Promise((resolve, reject) => {
        const payload = body ? JSON.stringify(body) : null
        const req = http.request({
            host: '127.0.0.1',
            port,
            path: route,
            method,
            headers: payload ? { 'Content-Type': 'application/json', 'Content-Length': Buffer.byteLength(payload) } : {}
        }, (res) => {
            let data = ''
            res.on('data', (c) => (data += c))
            res.on('end', () => {
                try { resolve(JSON.parse(data)) } catch (e) { reject(new Error('后端响应解析失败: ' + String(data).slice(0, 200))) }
            })
        })
        req.on('error', reject)
        if (payload) req.write(payload)
        req.end()
    })
}

/**
 * 构建编码器参数；默认 GPU NVENC，nvenc 不可用（驱动/无 NVIDIA）时回退 CPU x265
 * @returns { args: string[], name: 'nvenc'|'x265' }
 */
function buildEncoderArgs(encoder, crf, x265Params) {
    if (encoder === 'nvenc') {
        return {
            args: ['-c:v', 'hevc_nvenc', '-preset', 'p5', '-rc', 'vbr', '-cq', String(crf), '-b:v', '0', '-tag:v', 'hvc1'],
            name: 'nvenc'
        }
    }
    return {
        args: ['-c:v', 'libx265', '-preset', 'medium', '-crf', String(crf), '-tag:v', 'hvc1', '-x265-params', x265Params],
        name: 'x265'
    }
}

// ============================================================
//  链路 1：直接转（单层色调映射，图片 ICC 增益式）
// ============================================================
/**
 * SDR 视频 → HDR10（单层色调映射，对应图片 jpg_icc）
 * 用 Kotlin 后端 applyHdrTransform（无自动伽马）逐帧变换 → 16-bit PAM → ffmpeg 编码。
 * 全套图片参数：hdrIntensity×fineTuneBrightness=曝光、gamma、rgbAdjustment。
 * opts.backendPort: Kotlin 后端端口（必须已启动）
 */
async function convertVideoDirect(inputPath, outputPath, settings, opts, onProgress) {
    return convertVideoFrames(inputPath, outputPath, settings, { ...(opts || {}), transformMode: 'transform' }, onProgress)
}

// ============================================================
//  链路 2：逐帧增益图转换
// ============================================================
/**
 * SDR 视频 → 解码逐帧 → /video-frame 重建线性 HDR → PAM → HDR10
 * settings.hdrIntensity: 增益图 EV（默认 1.5）
 * settings.gamma: 高光掩膜曲线（默认 0.9）
 * settings.crf: x265 质量（默认 20）
 * settings.maxWidth: 处理宽度上限（0=原始分辨率，默认 0 保持源尺寸；>0 时把帧宽压到该上限省内存/耗时）
 * opts.backendPort: Kotlin 后端端口（必须已启动）
 */
async function convertVideoFrames(inputPath, outputPath, settings, opts, onProgress) {
    const backendPort = opts.backendPort
    if (!backendPort) throw new Error('Kotlin 后端未就绪（需要 /video-frame）')
    const info = await probeVideo(inputPath)
    const crf = settings.crf || 20
    const hdrIntensity = settings.hdrIntensity || 1.5
    const gamma = settings.gamma || 0.9
    const fineTuneBrightness = settings.fineTuneBrightness != null ? settings.fineTuneBrightness : 1.0
    const rgbAdjustment = settings.rgbAdjustment || { red: 1.0, green: 1.0, blue: 1.0 }
    const transformMode = (opts && opts.transformMode) || 'gainmap'
    const modeLabel = transformMode === 'transform' ? '单层 HDR 变换（ICC 增益式）' : '增益图重建'
  // 白点 / 峰值（用户可调）：peak=PAM归一化峰值(白点倍率)、npl=峰值亮度、白点=npl/peak
  const whiteNits = settings.whiteNits || DEFAULT_WHITE_NITS
  const peakNits = settings.peakNits || DEFAULT_PEAK_NITS
  const peak = peakNits / whiteNits
  const npl = peakNits
  const maxCll = peakNits
    const outBase = outputPath.replace(/\.[^.]+$/, '')
    const tmpDir = outBase + '_hdr_frames'
    fs.mkdirSync(tmpDir, { recursive: true })

    // 1) 解码为 PNG 帧序列（可选限宽）
    const scaleVf = settings.maxWidth && settings.maxWidth > 0
        ? ['-vf', `scale='min(${settings.maxWidth},iw)':-2`]
        : []
    const extractArgs = [
        '-y', '-nostats', '-i', inputPath,
        ...scaleVf,
        '-f', 'image2', '-start_number', '0',
        path.join(tmpDir, 'frame_%06d.png')
    ]
    onProgress(0.0, '正在解码视频帧…')
    await runFFmpeg(extractArgs)

    // 枚举实际帧数
    const frames = fs.readdirSync(tmpDir).filter((f) => /^frame_\d{6}\.png$/.test(f)).sort()
    const total = frames.length
    if (!total) throw new Error('视频解码失败：未生成任何帧')
    const fps = info.fps || 30

    // 2) 逐帧重建线性 HDR → 16-bit PAM（有限并发池，按帧号回写保证顺序不乱）
    //    后端 /video-frame 不受信号量限流，主进程用 worker 池限并发。
    //    PAM 由后端直接写盘（outputPath），不再经 base64 往返，避免大块数据编解码开销。
    onProgress(0.0, `逐帧${modeLabel} 0/${total}…`)
    let completed = 0
    const writePam = async (i) => {
        const framePath = path.join(tmpDir, frames[i])
        const pamPath = path.join(tmpDir, 'hdr_' + String(i).padStart(6, '0') + '.pam')
        const resp = await httpJson(backendPort, 'POST', '/video-frame', {
            inputPath: framePath,
            settings: { hdrIntensity, gamma, fineTuneBrightness, rgbAdjustment, outputFormat: 'jpg' },
            peak,
            mode: transformMode,
            outputPath: pamPath
        })
        if (!resp || !resp.ok) {
            // 响应未确认 ok：按旧协议回退（或有 base64）再兼容，否则报错
            if (resp && resp.pamBase64) {
                fs.writeFileSync(pamPath, Buffer.from(resp.pamBase64, 'base64'))
            } else {
                throw new Error('后端逐帧重建失败: ' + ((resp && resp.message) || '无响应'))
            }
        }
        completed++
        onProgress(completed / total, `逐帧${modeLabel} ${completed}/${total}…`)
    }
    // 并发池：固定 worker 数，idx 指针推进取帧；任一帧失败即整体失败（与串行语义一致）
    const concurrency = Math.max(1, Math.min(FRAME_CONCURRENCY, total))
    let idx = 0
    await Promise.all(
        Array.from({ length: concurrency }, async () => {
            while (idx < total) {
                const i = idx++
                await writePam(i)
            }
        })
    )

    // 3) 编码 HDR10
    onProgress(0.0, '正在编码 HDR10 视频…')
    const vf =
        `zscale=in_range=full:pin=bt709:tin=linear:npl=${npl}:` +
        'p=bt2020:t=smpte2084:m=bt2020nc:r=limited,format=yuv420p10le'
    const wantNvenc = settings.encoder !== 'x265' // 默认 GPU 编码
    const x265 = `colorprim=bt2020:transfer=smpte2084:colormatrix=bt2020nc:${MASTER_DISPLAY}:max-cll=${Math.round(maxCll)},400:repeat-headers=1:profile=main10`
    const silentOut = path.join(tmpDir, 'silent_hdr.mp4')
    const durationUs = Math.round((total / fps) * 1000000)
    const buildArgs = (enc) => [
        '-y', '-nostats', '-framerate', String(fps), '-start_number', '0',
        '-i', path.join(tmpDir, 'hdr_%06d.pam'),
        '-vf', vf,
        ...enc.args,
        // 显式声明流级色彩属性 → mp4 容器写 colr(nclx) 盒
        '-color_primaries', 'bt2020', '-color_trc', 'smpte2084', '-colorspace', 'bt2020nc', '-color_range', 'tv',
        '-an',
        '-progress', 'pipe:1',
        silentOut
    ]
    let enc = buildEncoderArgs(wantNvenc ? 'nvenc' : 'x265', crf, x265)
    try {
        await runFFmpeg(buildArgs(enc), { onProgress: (p) => onProgress(p, '正在编码 HDR10 视频…'), durationUs })
    } catch (err) {
        // GPU 编码失败 → 回退 CPU x265
        if (enc.name === 'nvenc') {
            enc = buildEncoderArgs('x265', crf, x265)
            await runFFmpeg(buildArgs(enc), { onProgress: (p) => onProgress(p, '正在编码 HDR10 视频…'), durationUs })
        } else {
            throw err
        }
    }

    // 4) 合并原音频（尽力而为，失败则保留无声版）
    try {
        fs.mkdirSync(path.dirname(outputPath), { recursive: true })
        await runFFmpeg([
            '-y', '-nostats', '-i', silentOut, '-i', inputPath,
            '-map', '0:v:0', '-map', '1:a:0?',
            '-c:v', 'copy', '-c:a', 'aac', '-b:a', '160k', '-shortest',
            outputPath
        ])
    } catch (e) {
        fs.copyFileSync(silentOut, outputPath)
    }

    // 5) 注入 mdcv / clli 容器盒（Chromium demuxer 依赖）
    injectHdrBoxes(outputPath, { maxCll: Math.round(maxCll), maxFall: 400 })

    // 清理临时目录
    try { fs.rmSync(tmpDir, { recursive: true, force: true }) } catch (e) { /* ignore */ }
    return { outputPath, info, encoder: enc.name }
}

/**
 * 提取视频首帧 → JPEG data URL + 临时帧文件路径
 * @returns { dataUrl, framePath } framePath 供图片 HDR 链路（Kotlin /preview）做首帧 HDR 预览
 */
async function extractFirstFrame(inputPath) {
    // 固定临时路径（每次覆盖，不堆积）；os.tmpdir 由系统清理
    const framePath = path.join(os.tmpdir(), 'hdr_electron_video_frame.jpg')
    await runFFmpeg([
        '-y', '-nostats', '-ss', '0',
        '-i', inputPath,
        '-frames:v', '1',
        '-vf', 'scale=1280:-2',
        framePath
    ])
    const data = await fs.promises.readFile(framePath)
    return { dataUrl: 'data:image/jpeg;base64,' + data.toString('base64'), framePath }
}

/**
 * 提取视频指定时间点的一帧 → JPEG data URL + 临时帧文件路径
 * 供「拖动进度条 → 立即生成该处 HDR 压缩预览图」使用
 * @param {string} inputPath 视频路径
 * @param {number} timeSeconds 目标时间（秒）
 * @returns { dataUrl, framePath, time }
 */
async function extractFrameAt(inputPath, timeSeconds) {
    const t = Math.max(0, Number(timeSeconds) || 0)
    const framePath = path.join(os.tmpdir(), 'hdr_electron_video_frame_at.jpg')
    // -ss 放在 -i 之前 = 快速 seek（先跳到关键帧再解码到目标时间），比逐帧解码快很多
    await runFFmpeg([
        '-y', '-nostats', '-ss', String(t),
        '-i', inputPath,
        '-frames:v', '1',
        '-vf', 'scale=1280:-2',
        framePath
    ])
    const data = await fs.promises.readFile(framePath)
    return { dataUrl: 'data:image/jpeg;base64,' + data.toString('base64'), framePath, time: t }
}

module.exports = {
    FFMPEG,
    FFPROBE,
    probeVideo,
    convertVideoDirect,
    convertVideoFrames,
    extractFirstFrame,
    extractFrameAt,
    cancelAllFFmpeg
}
