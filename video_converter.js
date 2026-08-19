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

/**
 * 探测视频的「可视高度」与「编码高度（coded_height）」。
 * NVENC 会按 32 像素对齐把高度补上去（如 2160→2176、1080→1088），
 * 导致 coded ≠ visible，部分渲染器（如 Wallpaper Engine）会因此把补边的
 * 非 16:9 编码框显示成上下黑边。这里返回两者用于归一判断。
 * @returns {{ height:number, codedHeight:number }}
 */
async function probeCodedHeight(inputPath) {
    const j = await ffprobeJson(['-show_streams', inputPath])
    const vs = (j.streams || []).find((s) => s.codec_type === 'video')
    if (!vs) return { height: 0, codedHeight: 0 }
    return { height: vs.height || 0, codedHeight: vs.coded_height || vs.height || 0 }
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

/** 主进程内 HTTP 请求，返回原始二进制 Buffer（如后端 /video-frame 返回的 PAM）。
 *  避免 JSON/base64 编解码，响应体直接按字节收集。 */
function httpBinary(port, method, route, body) {
    return new Promise((resolve, reject) => {
        const payload = body ? JSON.stringify(body) : null
        const req = http.request({
            host: '127.0.0.1',
            port,
            path: route,
            method,
            headers: payload ? { 'Content-Type': 'application/json', 'Content-Length': Buffer.byteLength(payload) } : {}
        }, (res) => {
            const chunks = []
            res.on('data', (c) => chunks.push(c))
            res.on('end', () => {
                const buf = Buffer.concat(chunks)
                if (res.statusCode >= 400) {
                    reject(new Error('后端请求失败 ' + res.statusCode + ': ' + buf.toString('utf8').slice(0, 200)))
                } else {
                    resolve(buf)
                }
            })
        })
        req.on('error', reject)
        if (payload) req.write(payload)
        req.end()
    })
}

/**
 * 构建编码器参数。
 *
 * 重要澄清：编码器与「CUDA/GPU 加速」是两码事，二者相互独立——
 *  - 视频逐帧的 HDR 重建（色调映射/增益图）由 Kotlin 后端完成：目前为 JVM CPU
 *    计算（靠帧级并发提速；「视频逐帧重建 CUDA 化」是 MEMORY.md 待办，尚未实现，
 *    现有 CUDA 内核均为 8-bit 输出，视频链路需要的 16-bit PAM 内核还没写）。
 *    这一步与下面选哪个编码器无关。
 *  - 这里只是选择「收尾把重建好的帧压成 HEVC/AV1」的**编码器**，可独立在
 *    硬件编码（hevc_nvenc / av1_nvenc）与软件编码（libx265 / libaom-av1）之间选，
 *    并不等于「用 CUDA 加速」。
 *
 * 支持的编码器（name -> ffmpeg 编码器）：
 *  - nvenc     -> hevc_nvenc （HEVC，NVIDIA 硬件，默认）
 *  - x265      -> libx265    （HEVC，CPU 软件）
 *  - av1       -> libaom-av1 （AV1，CPU 软件）
 *  - av1_nvenc -> av1_nvenc  （AV1，NVIDIA 硬件，需 RTX 40 系列及以上）
 * @returns { args: string[], name: string }
 */
function buildEncoderArgs(encoder, crf, x265Params) {
    if (encoder === 'nvenc') {
        return {
            args: ['-c:v', 'hevc_nvenc', '-preset', 'p5', '-rc', 'vbr', '-cq', String(crf), '-b:v', '0', '-tag:v', 'hvc1'],
            name: 'nvenc'
        }
    }
    if (encoder === 'av1_nvenc') {
        return {
            args: ['-c:v', 'av1_nvenc', '-preset', 'p5', '-rc', 'vbr', '-cq', String(crf), '-b:v', '0', '-tag:v', 'av01'],
            name: 'av1_nvenc'
        }
    }
    if (encoder === 'av1') {
        // libaom-av1 软件 AV1：-crf + -b:v 0 走恒定质量；-cpu-used 权衡速度/效率
        return {
            args: ['-c:v', 'libaom-av1', '-crf', String(crf), '-b:v', '0', '-cpu-used', '5', '-row-mt', '1', '-tag:v', 'av01'],
            name: 'av1'
        }
    }
    // 默认 x265（HEVC 软件）
    return {
        args: ['-c:v', 'libx265', '-preset', 'medium', '-crf', String(crf), '-tag:v', 'hvc1', '-x265-params', x265Params],
        name: 'x265'
    }
}

/** 探测 ffmpeg 是否可用指定编码器（管道化后输入流不可重放，必须先探测再选编码器）。
 *  查 -encoders 列表 + 实际试编码一帧（后者能捕获驱动/初始化问题，
 *  避免编码器启动后才失败导致管道流无法回退）。 */
function encoderAvailable(encName) {
    return new Promise((resolve) => {
        const listProc = spawn(FFMPEG, ['-hide_banner', '-encoders'], { windowsHide: true })
        let out = ''
        listProc.stdout.on('data', (d) => (out += d.toString()))
        listProc.stderr.on('data', (d) => (out += d.toString()))
        listProc.on('close', async () => {
            if (!out.includes(encName)) return resolve(false)
            // 实际试编码 1 帧（null 输出）：驱动不可用/初始化失败时会非 0 退出。
            // 注意尺寸必须满足 NVENC 最小限制（2x2 会被拒导致误判 nvenc 不可用），
            // 用 320x240；yuv420p10le 模拟真实 HDR 转换的 10-bit 输入路径。
            const probeArgs = [
                '-hide_banner', '-loglevel', 'error',
                '-f', 'lavfi', '-i', 'color=black:s=320x240:d=0.04,format=yuv420p10le',
                '-frames:v', '1', '-c:v', encName, '-f', 'null', '-'
            ]
            const probeProc = spawn(FFMPEG, probeArgs, { windowsHide: true })
            let probeErr = ''
            probeProc.stderr.on('data', (d) => (probeErr += d.toString()))
            probeProc.on('error', () => resolve(false))
            probeProc.on('close', (code) => resolve(code === 0))
        })
        listProc.on('error', () => resolve(false))
    })
}

/**
 * 归一化 NVENC 输出的编码高度补边（coded != visible 时触发）。
 *
 * 背景：hevc_nvenc 会让 coded_height 按 32 像素对齐，把 2160 补到 2176（1080→1088）。
 * 结果 coded 帧宽高比不再是 16:9（如 3840:2176 ≈ 30:17），而部分渲染器
 * （如 Wallpaper Engine）会把这段补边的编码框显示成上下黑边（尤其 16:10 屏更明显）。
 * 而 libx265 不会补边（coded == 可视高度），所以这里用 libx265 把 silentOut
 * 重新编码一次：解码阶段 ffmpeg 默认会应用 conformance window 裁剪出可视 2160 行，
 * 再经 x265 编码即得到 coded == visible 的干净 16:9 文件。
 *
 * @param {string} silentPath 编码器产出的无声 HDR mp4（可能带 coded 补边）
 * @param {object} opts { npl, maxCll } 用于重建 HDR 编码参数
 * @returns {Promise<string>} 归一化后的文件路径（无补边则原样返回）
 */
async function normalizeCodedHeight(silentPath, { npl, maxCll } = {}) {
    const { height, codedHeight } = await probeCodedHeight(silentPath)
    if (!height || height <= 0 || codedHeight === height) {
        // 无补边（如 x265 产物 / 高度恰好 32 对齐）→ 无需归一
        return silentPath
    }
    console.log('[video] 检测到 NVENC 编码高度补边 ' + height + '→' + codedHeight +
        '，执行归一化重编码（消除黑边）…')
    const normOut = silentPath.replace(/\.mp4$/i, '_norm.mp4')
    const x265 = `colorprim=bt2020:transfer=smpte2084:colormatrix=bt2020nc:${MASTER_DISPLAY}:max-cll=${Math.round(maxCll || DEFAULT_PEAK_NITS)},400:repeat-headers=1:profile=main10`
    // 解码默认应用 conformance crop → 拿到可视 height 行 → x265 按该高度编码（coded==visible）
    await runFFmpeg([
        '-y', '-nostats', '-i', silentPath,
        '-c:v', 'libx265', '-preset', 'medium', '-crf', '18', '-tag:v', 'hvc1', '-x265-params', x265,
        '-color_primaries', 'bt2020', '-color_trc', 'smpte2084', '-colorspace', 'bt2020nc', '-color_range', 'tv',
        '-an',
        normOut
    ])
    return normOut
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

    // 1) 解码为 PNG 帧序列（可选限宽）。优先尝试 CUDA 硬件解码（NVDEC），
    //    失败自动回退 CPU 软解。回退时打印原因，便于排查为何 GPU 未生效。
    const scaleVf = settings.maxWidth && settings.maxWidth > 0
        ? ['-vf', `scale='min(${settings.maxWidth},iw)':-2`]
        : []
    const extractArgs = (hw) => [
        '-y', '-nostats', ...(hw ? ['-hwaccel', 'cuda'] : []), '-i', inputPath,
        ...scaleVf,
        '-f', 'image2', '-start_number', '0',
        path.join(tmpDir, 'frame_%06d.png')
    ]
    onProgress(0.0, '正在解码视频帧…')
    // cuvid 支持的输入编码（对应 ffmpeg -decoders 里的 *cuvid 解码器）；
    // 不支持的编码直接软解，避免先失败重跑浪费一倍解码时间。
    const cuvidCodecs = new Set(['h264', 'hevc', 'av1', 'mpeg2video', 'mpeg1video', 'mpeg4', 'vc1', 'vp8', 'vp9', 'mjpeg'])
    const codecName = info.codec || ''
    const useCuda = cuvidCodecs.has(codecName)
    try {
        if (useCuda) {
            await runFFmpeg(extractArgs(true))
            console.log('[video] 解码使用 CUDA 硬件加速（' + codecName + '）')
        } else {
            await runFFmpeg(extractArgs(false))
            console.log('[video] 输入编码 ' + codecName + ' 无 cuvid 解码器，使用 CPU 软解')
        }
    } catch (e) {
        // CUDA 解码失败（驱动/卡不支持等）→ 回退 CPU 软解
        console.warn('[video] CUDA 解码失败，回退 CPU 软解: ' + ((e && e.message) || e))
        await runFFmpeg(extractArgs(false))
    }

    // 枚举实际帧数
    const frames = fs.readdirSync(tmpDir).filter((f) => /^frame_\d{6}\.png$/.test(f)).sort()
    const total = frames.length
    if (!total) throw new Error('视频解码失败：未生成任何帧')
    const fps = info.fps || 30

    // 2) 编码器准备：管道化后 PAM 流不可重放，必须先探测编码器可用性再启动
    //    （首选编码器不可用时按降级链自动回退，避免启动后才失败导致无法回退）。
    //
    //    编码器与「CUDA 加速」无关：逐帧 HDR 重建由 Kotlin 后端完成，目前为 JVM CPU
    //    计算（「视频逐帧重建 CUDA 化」是待办，尚未实现），与这里选哪个编码器无关；
    //    编码器只是收尾压片方案，可在 HEVC/AV1、硬编/软编之间独立选择。
    //    默认偏好 x265（HEVC 软件，coded==visible 无黑边补边问题）；nvenc 为快但会
    //    按 32 对齐补边需归一，故不作为默认。
    //    降级链：x265；nvenc→x265；av1_nvenc→av1→x265；av1→x265。
    const vf =
        `zscale=in_range=full:pin=bt709:tin=linear:npl=${npl}:` +
        'p=bt2020:t=smpte2084:m=bt2020nc:r=limited,format=yuv420p10le'
    const x265 = `colorprim=bt2020:transfer=smpte2084:colormatrix=bt2020nc:${MASTER_DISPLAY}:max-cll=${Math.round(maxCll)},400:repeat-headers=1:profile=main10`
    const silentOut = path.join(tmpDir, 'silent_hdr.mp4')
    const durationUs = Math.round((total / fps) * 1000000)
    // 默认 x265（首选、无黑边补边）；其余编码器仅在用户显式选择时生效
    const defaultEncoder = 'x265'
    // 编码器 -> ffmpeg 探测名 -> 降级链
    const FFMpegProbeName = { nvenc: 'hevc_nvenc', 'av1_nvenc': 'av1_nvenc', av1: 'libaom-av1' }
    const fallbackChain = {
        x265: ['x265'],
        nvenc: ['nvenc', 'x265'],
        'av1_nvenc': ['av1_nvenc', 'av1', 'x265'],
        av1: ['av1', 'x265']
    }[(settings.encoder || defaultEncoder)] || ['x265']
    let enc = null
    for (const encName of fallbackChain) {
        if (encName === 'x265') { enc = buildEncoderArgs('x265', crf, x265); break }
        if (await encoderAvailable(FFMpegProbeName[encName])) { enc = buildEncoderArgs(encName, crf, x265); break }
    }
    if (!enc) enc = buildEncoderArgs('x265', crf, x265)

    // 3) 启动编码器：从 stdin 读 PAM 序列（image2pipe），逐帧边重建边喂入 ——
    //    PAM 不再落盘（省最大 SSD 写入 ~50MB/帧），CPU 也不再等磁盘写。
    //    注意：image2pipe 不支持 -start_number（那是 image2 文件序列的选项），
    //    传了会导致 "Option start_number not found" 直接退出。
    onProgress(0.0, `逐帧${modeLabel} 0/${total}…`)
    const encArgs = [
        '-y', '-nostats',
        // 用 pam_pipe（piped pam sequence）而非 image2pipe：pam_pipe 明确知道
        // 输入是 PAM 格式，不依赖探测——image2pipe 需探测具体图片格式，管道空时
        // 报 "Could not find codec parameters ... (Video: none, none)" 直接退出。
        '-f', 'pam_pipe', '-framerate', String(fps),
        '-i', 'pipe:0',
        '-vf', vf,
        ...enc.args,
        // 显式声明流级色彩属性 → mp4 容器写 colr(nclx) 盒
        '-color_primaries', 'bt2020', '-color_trc', 'smpte2084', '-colorspace', 'bt2020nc', '-color_range', 'tv',
        '-an',
        '-progress', 'pipe:1',
        silentOut
    ]
    // 编码器**延迟启动**：第一次喂帧时才 spawn（此时 PAM 已到手，管道立即有数据）。
    // 若启动即探测 pipe:0 而管道为空，ffmpeg 会报
    // "Could not find codec parameters ... (Video: none, none): unknown codec" 直接退出。
    let allFramesFed = false   // 逐帧全部喂入后，进度条切换为编码进度
    let feedError = null       // 编码器失败/逐帧失败时的中止信号（worker 快速退出）
    let encoder = null   // { proc, writeStdin, done, failed }
    const pendingWrites = new Set()
    const flushPendingWrites = () => {
        for (const w of pendingWrites) w()
        pendingWrites.clear()
    }
    const startEncoder = () => {
        const proc = spawn(FFMPEG, encArgs, { windowsHide: true, cwd: RUN_CWD })
        activeFFmpeg.add(proc)
        let encStderr = ''
        let encFailed = false
        // 编码器退出后写 stdin 会触发 'error'（write EOF）——不监听会变成
        // uncaughtException 直接崩溃主进程。写失败只是"编码器已退出"的副产物，
        // 真实错误由 close 事件统一报告，这里只负责唤醒挂起的写入。
        proc.stdin.on('error', () => { encFailed = true; flushPendingWrites() })
        const done = new Promise((resolve, reject) => {
            // 必须消费 stdout（-progress pipe:1），否则缓冲区填满会阻塞 ffmpeg
            proc.stdout.on('data', (d) => {
                const text = d.toString()
                const m = text.match(/out_time_us=(\d+)/)
                // 逐帧重建完成前进度条反映逐帧进度；完成后才显示编码进度（避免跳变）
                if (m && durationUs && allFramesFed) {
                    const p = Math.min(1, parseInt(m[1], 10) / durationUs)
                    onProgress(p, '编码中…')
                }
            })
            proc.stderr.on('data', (d) => { encStderr += d.toString() })
            proc.on('error', (err) => { encFailed = true; feedError = err; flushPendingWrites(); reject(err) })
            proc.on('close', (code) => {
                activeFFmpeg.delete(proc)
                encFailed = true
                flushPendingWrites()
                if (code === 0 && !feedError) resolve()
                else {
                    const err = new Error('ffmpeg 编码退出码 ' + code + '\n' + encStderr.slice(-800))
                    feedError = feedError || err
                    reject(feedError)
                }
            })
        })
        // 写入 stdin：写回调**忽略 write EOF**，由 close 事件报告真实错误；
        // 编码器已死时直接跳过写入。
        const writeStdin = (buf) => new Promise((resolve) => {
            if (encFailed) return resolve()
            let doneFlag = false
            const finish = () => { if (!doneFlag) { doneFlag = true; pendingWrites.delete(finish); resolve() } }
            pendingWrites.add(finish)
            try {
                proc.stdin.write(buf, finish)
            } catch (e) {
                encFailed = true
                finish()
            }
        })
        return { proc, writeStdin, done, failed: () => encFailed }
    }
    // 乱序帧缓冲：并发完成无序，按序号顺序喂入编码器
    const frameBuf = new Map()   // index -> PAM Buffer
    let nextIdx = 0
    const feedPam = async (i, pam) => {
        if (feedError) throw feedError
        frameBuf.set(i, pam)
        // 编码器延迟启动：当轮到该帧写入时才 spawn（保证管道立刻有数据可探测）
        if (!encoder && frameBuf.has(nextIdx)) {
            encoder = startEncoder()
        }
        while (frameBuf.has(nextIdx)) {
            const buf = frameBuf.get(nextIdx)
            frameBuf.delete(nextIdx)
            nextIdx++
            try {
                await encoder.writeStdin(buf)
            } catch (err) {
                feedError = err
                throw err
            }
        }
    }
    // 逐帧重建（并发池）：返回的 PAM 直接按序喂入编码器，不再写盘
    let completed = 0
    const processFrame = async (i) => {
        const framePath = path.join(tmpDir, frames[i])
        const pam = await httpBinary(backendPort, 'POST', '/video-frame', {
            inputPath: framePath,
            settings: { hdrIntensity, gamma, fineTuneBrightness, rgbAdjustment, outputFormat: 'jpg' },
            peak,
            mode: transformMode
        })
        if (!pam || pam.length === 0) throw new Error('后端逐帧重建失败: 空响应')
        await feedPam(i, pam)
        completed++
        onProgress(completed / total, `逐帧${modeLabel} ${completed}/${total}…`)
    }
    const concurrency = Math.max(1, Math.min(FRAME_CONCURRENCY, total))
    let idx = 0
    try {
        await Promise.all(
            Array.from({ length: concurrency }, async () => {
                while (idx < total && !feedError) {
                    const i = idx++
                    await processFrame(i)
                }
            })
        )
        // 所有 PAM 已喂入 → 关闭 stdin，等编码器消费完剩余帧并收尾
        allFramesFed = true
        if (encoder) {
            if (!encoder.failed()) encoder.proc.stdin.end()
            await encoder.done
        }
    } catch (err) {
        // 任一环节失败：终止编码器，清理
        if (encoder) {
            try { encoder.proc.stdin.destroy() } catch (e) { /* ignore */ }
            try { encoder.proc.kill() } catch (e) { /* ignore */ }
            activeFFmpeg.delete(encoder.proc)
        }
        throw err
    }
    onProgress(1, '编码完成')

    // 3.5) 归一化编码高度补边（coded != visible → 部分渲染器如 Wallpaper Engine
    //      会把补边的非 16:9 编码框显示成上下黑边）。
    //      实测只有 hevc_nvenc 会按 32 对齐把 2160→2176（1080→1088）；
    //      libx265 与 AV1 系（libaom-av1 / av1_nvenc）都是 coded == visible，无需处理。
    //      归一用同属 HEVC 的 libx265 重新编码（解码时按 conformance crop 取可视高度）。
    let muxSource = silentOut
    if (enc.name === 'nvenc') {
        onProgress(1, '正在归一化编码高度（消除黑边）…')
        try {
            muxSource = await normalizeCodedHeight(silentOut, { maxCll })
        } catch (e) {
            console.warn('[video] 编码高度归一化失败，沿用原产物: ' + ((e && e.message) || e))
        }
    }

    // 4) 合并原音频（尽力而为，失败则保留无声版）
    try {
        fs.mkdirSync(path.dirname(outputPath), { recursive: true })
        await runFFmpeg([
            '-y', '-nostats', '-i', muxSource, '-i', inputPath,
            '-map', '0:v:0', '-map', '1:a:0?',
            '-c:v', 'copy', '-c:a', 'aac', '-b:a', '160k', '-shortest',
            outputPath
        ])
    } catch (e) {
        fs.copyFileSync(muxSource, outputPath)
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
