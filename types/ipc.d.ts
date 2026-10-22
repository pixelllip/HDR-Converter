/**
 * HDR Converter — Electron IPC 契约（单一事实来源 / single source of truth）
 * ============================================================================
 *
 * 本文件描述「渲染进程 ↔ preload ↔ 主进程 ↔ Rust 后端」之间实际传输的数据形状。
 * 字段名一律以**实际传输的 JSON key** 为准（camelCase）。Rust 侧靠 serde rename 对齐：
 *   backend/rust/src/server.rs  JsonSettings / ConvertReq / PreviewReq /
 *                               VideoFrameReq / BatchJob / BatchConvertReq …
 *   backend/rust/src/lib.rs     analyze-eclipsa / read-hdr-meta 的 println! JSON
 *
 * 【为什么需要这个文件】
 * 现状是所有 payload 都写成 `const { a, b } = payload || {}`，**既无运行时校验也无编译期校验**；
 * 同一个 `settings` 对象被 main.js 透传给 Rust、又被 video_converter.js 用 `||` 兜默认值，
 * 字段拼写错误只会静默变成 undefined。这里把契约集中写一次。
 *
 * 【三条必须知道的既有行为（都是实测，不是假想）】
 *
 * 1) `main.js` 的 `httpJson()` **不检查 HTTP 状态码**，任何响应都会 resolve。
 *    因此后端 500 的 `{"error":"…"}` 会作为**已 resolve 的正常值**回到渲染进程。
 *    受影响的路由：`/preview`（500 预览失败）——
 *    所以 `PreviewResult` 带上了 `BackendErrorBody` 联合分支。
 * 后果（现存隐患，本文件只做记录）：
 *      - `image.html` L644 只判断 `result.dataUrl`，拿不到就退化成"后端未返回图像"；
 *      - `image.html` L794 用 `result.success !== false` 判定成功 —— 也就是说
 *        `/convert` 若返回错误体也会被当成成功（不过 `/convert` 实际永不 500，见下）。
 *
 * 2) `/convert`（图片）**从不返回 HTTP 错误**：失败也是 200 + `ConvertResp{success:false}`，
 *    见 server.rs L328-412。所以 `ConvertImageResponse` 不需要错误联合。
 *
 * 3) `/cancel` 与 `/batch/cancel` 返回的是 `{"ok":"true"}` —— **字符串 "true"**，不是布尔；
 *    而 main.js 自己在 `batch-cancel-images` 的空数组短路分支返回的是布尔 `{ok:true}`。
 *    渲染进程目前忽略该返回值，但类型必须如实反映 `boolean | string`。
 *
 * 【本文件是 ambient script】
 * 没有顶层 import/export，因此 `HdrIpc.*` 全局可见：现有 JS 可用 JSDoc
 * `/** @type {HdrIpc.ImageSettings} *\/` 引用，未来 .ts 文件可直接使用。
 */

declare namespace HdrIpc {
  // ==========================================================================
  //  0. 通用
  // ==========================================================================

  /** 后端 500 响应体（main.js httpJson 不检查状态码，会 resolve 成这个形状） */
  interface BackendErrorBody {
    error: string
  }

  /**
   * preload 的 `on*Progress` 返回的取消订阅函数。
   * ⚠️ 现状：`views/image.html` L830/L993 与 `views/video.html` L1605 都**丢弃**了返回值，
   * 全仓库没有任何 removeListener 调用 —— 重新加载页面脚本会叠加监听器（无 teardown 路径）。
   */
  type Unsubscribe = () => void

  /** 对话框/保存框被取消时统一返回 null */
  type Maybe<T> = T | null

  // ==========================================================================
  //  1. 信号枚举（与 video_converter.js 的映射表、views/hdr_preview_media.js 的
  //     TF_OPTIONS / GAMUT_ORDER 逐一对齐）
  // ==========================================================================

  /** 9 值传递函数表 —— video_converter.js `TF_TO_FFMPEG` / `TF_TO_ZIMG` 的键 */
  type TransferFunction =
    | 'pq' | 'hlg' | 'srgb' | 'rec709'
    | 'g22' | 'g28' | 'rec2020_10' | 'rec2020_12' | 'lin'

  /** 8 值色域表 —— video_converter.js `PRI_TO_FFMPEG` 的键（可被 ffmpeg/zscale 直接表达） */
  type Primaries = '709' | '470m' | '470bg' | '601' | '240m' | '2020' | 'p3' | 'film'

  /**
   * 11 值 UI 色域表 —— `views/hdr_preview_media.js` `GAMUT_ORDER`，
   * 与 video.html `#videoInputPrimaries` 选项一致。
   * 其中 'xyz' | '431' | '22' **编码器表达不了**：video_converter.js `resolveOutputPrimaries()`
   * 返回 null → 回退 BT.2020 并打一条进度消息。
   */
  type UiPrimaries = '709' | '470m' | '470bg' | '601' | '240m' | 'film' | '2020' | 'xyz' | '431' | 'p3' | '22'

  /** 视频链路的输入传递函数：video.html 把 auto 解析后只会送这两个值 */
  type VideoTransfer = 'pq' | 'hlg'

  type RgbAdjustment = {
    red: number
    green: number
    blue: number
  }

  // ==========================================================================
  //  2. 图片链路（views/image.html）
  // ==========================================================================

  /** 图片输出格式。Rust `OutputFormat::parse` 还接受 'hdr-png' | 'ultra-hdr' | 'ultrahdr' | 'uhdr' | 'jpeg' */
  type ImageOutputFormat = 'png' | 'jpg' | 'jpg_icc'

  /** 页面 `#outputFormatSelect` 的原始选项（'auto' 由 getEffectiveOutputFormat() 解析掉） */
  type ImageOutputFormatOption = 'auto' | ImageOutputFormat

  /** 图片 ICC 路径输出色域（`#outputGamut`，仅 png / jpg_icc 生效） */
  type ImageOutputGamut = '2020' | 'p3' | 'srgb'

  /**
   * 图片转换设置 —— `views/image.html` `getCurrentSettings()` 的完整返回值。
   * 7 个键全部必需、无 null/undefined。
   * 注意：页面**从不**发送 inputTransfer / inputPrimaries / primarySrgb / fineTuneBrightness
   * （那些字段 Rust 收，但只有视频链路用前两个）。RGB 通道已由「输出色域」取代。
   */
  interface ImageSettings {
    /** EV；= nitsToEv(peakNits, whiteNits) = log2(max(1,peakNits)/max(80,whiteNits))，未钳制（峰值/白点解耦后仍沿用该换算） */
    hdrIntensity: number
    /** #rolloff，range 0–1 step 0.01；1=白点到峰值，0=白点不提亮。ICC 与 Ultra HDR 共用 */
    rolloff: number
    /** 已解析：'auto' 不可能出现在这里 */
    outputFormat: ImageOutputFormat
    /** #outputGamut；仅 ICC 路径（png / jpg_icc）生效，Ultra HDR 主图跟随输入 */
    outputGamut: ImageOutputGamut
    /** #jpgQuality / 100 → 0.1–1.0 */
    quality: number
    /** #whiteNits，number 80–400，parseInt 失败兜 203 */
    whiteNits: number
    /** #hdrIntensity，range 400–1250，parseInt 失败兜 evToNits(1.3, whiteNits)；语义=画面最亮点亮度（目标上限），与白点解耦 */
    peakNits: number
  }

  /**
   * Rust `JsonSettings`（server.rs L75-100）接受的**全部**键，均为可选。
   * `ImageSettings` 是它的子集；这里单独列出「页面不发但后端认」的字段，便于将来透传。
   */
  interface JsonSettingsOptionalExtras {
    /** 'srgb'|'rec709'|'g22'|'g28'|'rec2020_10'|'rec2020_12'|'lin'|'pq'|'hlg'；null/'srgb' = 默认 */
    inputTransfer?: TransferFunction | null
    /** null/'709' = 默认 */
    inputPrimaries?: UiPrimaries | null
    primarySrgb?: boolean
    fineTuneBrightness?: number | null
  }

  /** 批量任务里的图片设置：`outputFormat` 被收窄成从**输出扩展名**反推的值（image.html L950-951） */
  type BatchImageSettings = Omit<ImageSettings, 'outputFormat'> & { outputFormat: 'png' | 'jpg' }

  /** `convert-image` 的 payload（image.html L788-793）。4 个键全部必需 */
  interface ConvertImageRequest {
    inputPath: string
    outputPath: string
    settings: ImageSettings
    /** Kotlin 时代遗留；前端只会送字面量 'auto' */
    backendPreference: 'auto'
  }

  /** `convert-preview` 的 payload（image.html L673）。注意页面**不传** mode */
  interface ConvertPreviewRequest {
    inputPath: string
    settings: ImageSettings
    /** main.js 会透传给后端 /preview；views/ 无任何调用点传它 */
    mode?: string
  }

  /** `convert-preview-data` 的 payload（video.html 动态预览用；views/ 目前无调用点） */
  interface ConvertPreviewDataRequest {
    /** 'data:image/png;base64,…' 或 'data:image/jpeg;base64,…'（main.js 靠前缀判扩展名） */
    dataUrl: string
    settings: ImageSettings
    mode?: string
  }

  /** `/batch/convert` 的单个任务（image.html L951/L953，`id` 在发送前被剥掉） */
  interface BatchJobPayload {
    inputPath: string
    outputPath: string
    settings: BatchImageSettings
  }

  /** `batch-convert-images` 的 payload；`maxConcurrent` 由 main.js 注入，页面不传 */
  interface BatchConvertImagesRequest {
    jobs: BatchJobPayload[]
  }

  /** `batch-cancel-images` 的 payload */
  interface BatchCancelImagesRequest {
    inputPaths: string[]
  }

  // ---------- 图片链路响应 ----------

  /** Rust `ConvertResp`（server.rs L220-231）。`/convert` 永不 500，故无需错误分支 */
  interface ConvertImageResponse {
    success: boolean
    outputPath?: string
    outputFormat?: string
    /** 取消时后端返回 '已取消'，image.html L803 精确比较这个字符串 */
    message?: string
    detectedColorSpace?: string
  }

  /** Rust `PreviewResp`（server.rs L233-241） */
  interface PreviewResponse {
    dataUrl?: string
    width: number
    height: number
    aspectRatio?: number
  }

  /** `/preview` 可能 500 → 错误体会被 resolve 回来 */
  type PreviewResult = PreviewResponse | BackendErrorBody

  /** Rust `BatchJobResult` */
  interface BatchJobResult {
    inputPath: string
    outputPath?: string
    success: boolean
    message?: string
  }

  /** Rust `BatchConvertResp`。`successCount`/`failCount` 由前端重新统计，未使用 */
  interface BatchConvertResponse {
    results: BatchJobResult[]
    successCount: number
    failCount: number
  }

  /** 选择输入文件夹后的扫描结果（main.js L371-389） */
  interface SelectInputFolderResult {
    folder: string
    files: string[]
  }

  /**
   * 取消类接口的返回值。
   * main.js 的 `batch-cancel-images` 空数组短路分支返回布尔 `{ok:true}`；
   * Rust `/cancel` 与 `/batch/cancel` 返回字符串 `{"ok":"true"}`（server.rs L416/L731）。
   */
  interface CancelResponse {
    ok: boolean | string
  }

  /** `open-output-folder`（main.js L407-421） */
  interface OpenOutputFolderResponse {
    ok: boolean
    message?: string
  }

  // ==========================================================================
  //  3. 后端状态
  // ==========================================================================

  /** `get-backend-status`（main.js L458-484）。`capacity`/`message` 渲染进程未使用 */
  interface BackendStatusResponse {
    /** 'cuda' | 'cpu' */
    method: string
    threads: number
    capacity: number
    message: string
    hasGpu: boolean
    gpuName: string
  }

  // ==========================================================================
  //  4. 视频链路（views/video.html + video_converter.js）
  // ==========================================================================

  type VideoEncoder = 'x265' | 'nvenc' | 'av1' | 'av1_nvenc'
  type VideoFormat = 'hdr10' | 'eclipsa'
  type WindowScheme = 'scene' | 'uniform'

  /** ST 2094-50 / Eclipsa 分窗参数 */
  interface EclipsaSettings {
    windowScheme: WindowScheme
    uniformWindows: number
    sceneThreshold: number
    minWindowSec: number
  }

  /**
   * 视频转换设置 —— `views/video.html` L1196-1229 的完整返回值。
   * 全部键必需、无 null；`eclipsa` 即使在 format='hdr10' 时也总是存在。
   */
  interface VideoSettings {
    /** = peakNits / whiteNits（**不是** EV，与图片链路的 hdrIntensity 语义不同） */
    hdrIntensity: number
    /** 字面量 1.0（video.html L1200 硬编码） */
    gamma: number
    /** 字面量 { red: 1, green: 1, blue: 1 }（video.html L1201 硬编码） */
    rgbAdjustment: RgbAdjustment
    whiteNits: number
    peakNits: number
    /** 已解析，不会是 'auto' */
    inputTransfer: VideoTransfer
    /** 已解析，不会是 'auto' */
    inputPrimaries: UiPrimaries
    outputTransfer: VideoTransfer
    outputPrimaries: 'p3' | '2020' | UiPrimaries
    /** #videoCrf，range 0–28；**无 `||` 兜底 → 可能 NaN** */
    crf: number
    /** #videoMaxWidth，0 = 保持源分辨率；**无 `||` 兜底 → 可能 NaN** */
    maxWidth: number
    encoder: VideoEncoder
    format: VideoFormat
    eclipsa: EclipsaSettings
  }

  /**
   * `analyze-eclipsa` 的参数（video.html `agtmAnalysisSettings()` L601-620）。
   * 与 VideoSettings 不同，这里的 `transfer` 在页面里是**字面量 'auto'**
   * （Rust CLI 另接受 'pq'|'hlg'|'sdr'，但该页面不走）。
   */
  interface EclipsaAnalysisSettings {
    whiteNits: number
    transfer: 'auto' | 'pq' | 'hlg' | 'sdr'
    peakNits: number
    outputPrimaries: 'p3' | '2020'
    eclipsa: EclipsaSettings
  }

  /** `convert-video` 的 payload（video.html L1243）。3 个键必需 */
  interface ConvertVideoRequest {
    inputPath: string
    outputPath: string
    settings: VideoSettings
  }

  /** `analyze-eclipsa-windows` 的 payload（video.html L641） */
  interface AnalyzeEclipsaWindowsRequest {
    inputPath: string
    settings: EclipsaAnalysisSettings
  }

  /**
   * 单窗口 2094-50 数据 —— 来自 `hdrconv analyze-eclipsa` 的 stdout JSON
   * （lib.rs L258-287 的 win_json）。
   */
  interface EclipsaWindow {
    /** 渲染端按时间查窗：`views/hdr_preview_media.js` L1071 */
    start_time: number
    /** 渲染端增益用：`views/hdr_preview_media.js` L1082 */
    raw: number
    /** video.html L654 取每窗最大亮度 */
    max_cll_nits: number
    start_frame: number
    end_frame: number
    end_time: number
    h_baseline: number
  }

  /** `analyze-eclipsa` 的 stdout JSON（lib.rs L274-287） */
  interface AnalyzeEclipsaWindowsResult {
    windows: EclipsaWindow[]
    frame_count: number
    fps: number
    gain_space: string
    /** 回填的源传函（auto → 探测结果） */
    transfer: 'pq' | 'hlg' | 'sdr'
  }

  /**
   * `probe-video` → `video_converter.js` `probeVideo()` L116-136。
   * `colorTransfer`/`colorPrimaries` 在源未标注时是**空字符串**（不是 undefined）。
   */
  interface ProbeVideoResult {
    width: number
    height: number
    duration: number
    fps: number
    frames: number
    codec: string
    pixFmt: string
    hasAudio: boolean
    colorTransfer: string
    colorPrimaries: string
  }

  /** `extract-video-first-frame` → video_converter.js `extractFirstFrame` */
  interface ExtractFrameResult {
    dataUrl: string
    framePath: string
  }

  /** `convert-video` 的 info —— video_converter.js L687-738 的 outInfo（probeVideo 结果 + 覆盖） */
  interface ConvertVideoInfo extends Partial<ProbeVideoResult> {
    /** 'hdr10' | 'eclipsa'；Eclipsa 附加成功时才写入 'eclipsa' */
    format?: VideoFormat
    /** 生效的输出色域（可能是回退后的 '2020'） */
    colorPrimaries?: string
    /** 生效的输出传函 */
    colorTransfer?: string
    /** HDR10 恒为 'tv' */
    colorRange?: string
    eclipsa?: {
      windows: unknown[]
      totalSei: number
    }
    /** Eclipsa 附加失败时置 true（回退 HDR10） */
    eclipsaFailed?: boolean
  }

  /**
   * `convert-video` 的返回值（main.js L790-843）。
   *
   * `encoder` 是**实际生效**的编码器（降级链的最终结果：如请求 nvenc 但 NVENC 不可用会降级 x265）。
   * 曾存在契约缺口：`video_converter.convertVideoDirect` 一直在返回它（video_converter.js L738），
   * 但 main.js 的 handler 只回 `{ success, outputPath, info }` 把它丢了，
   * 导致 video.html L1249 的 `r.encoder || currentEncoder()` 永远走 fallback、
   * 界面显示"请求的编码器"而非"真正用上的"。**已修复**：三个 return 分支都回传 encoder。
   * 取消路径不产生此字段，故仍为可选。
   */
  interface ConvertVideoResult {
    success: boolean
    outputPath?: string
    info?: ConvertVideoInfo
    /** 取消路径专用（main.js L838）*/
    canceled?: boolean
    message?: string
    /** 实际生效的编码器（降级后的结果）；取消路径不返回 */
    encoder?: VideoEncoder
  }

  // ==========================================================================
  //  5. 进度事件（主进程 → 渲染进程）
  // ==========================================================================

  /** `conversion-progress`（main.js L217-221）。`active` 前端未使用 */
  interface ConversionProgressEvent {
    /** 0..1 */
    value: number
    active: boolean
    message: string
  }

  /** `batch-progress`（main.js L498-506）。`current`/`running` 前端未使用 */
  interface BatchProgressEvent {
    total: number
    done: number
    failed: number
    current: string
    message: string
    running: boolean
    /** inputPath → 状态字符串；image.html L999 **不校验**就赋给 job.status */
    statuses: Record<string, string>
  }

  /** `video-progress`（main.js L795） */
  interface VideoProgressEvent {
    /** 0..1 */
    value: number
    message?: string
  }

  // ==========================================================================
  //  6. read-hdr-meta（CLI stdout JSON，hdr_meta.rs L622-637）
  // ==========================================================================

  /** `hdrconv read-hdr-meta` 的输出。⚠️ preload 暴露了它，但 `views/` 下**零调用点** */
  interface ReadHdrMetaResult {
    source: string
    codec: string
    colour: {
      /** CICP 数值码（hdr_meta.rs `cicp_from_name`） */
      colour_primaries: number | null
      transfer_characteristics: number | null
      matrix_coefficients: number | null
      primaries_name: string
      transfer_name: string
      matrix_name: string
    }
    /** mdcv 归一化后的键（hdr_meta.rs L521-547）：red_x…white_point_y 已 /10000，max/min_luminance 亦同 */
    mastering_display: Partial<Record<
      'red_x' | 'red_y' | 'green_x' | 'green_y' | 'blue_x' | 'blue_y' |
      'white_point_x' | 'white_point_y' | 'max_luminance' | 'min_luminance',
      number
    >>
    content_light_level: Partial<Record<'max_content' | 'max_average', number>>
    st2094_50_windows: unknown[]
    st2094_50_count: number
  }

  // ==========================================================================
  //  7. preload 暴露的 API（contextBridge → window.electronAPI）
  // ==========================================================================

  /**
   * `preload.js` 通过 `contextBridge.exposeInMainWorld('electronAPI', …)` 暴露的全部成员。
   *
   * `getSystemTheme` / `onSystemTheme` 已接线（main.js 的 systemPreferences + nativeTheme），
   * 见下方 `SystemTheme` 的格式说明 —— 那里的 `#` 前缀是踩过的坑。
   */
  interface ElectronAPI {
    // ---------- 同步（非 invoke 成员：webUtils 与 sendSync） ----------
    /** Electron 33 移除了 File.path，拖拽文件必须走它取真实路径 */
    getPathForFile(file: File): string

    // ---------- 图片 ----------
    selectInputImage(): Promise<Maybe<string>>
    selectInputImages(): Promise<Maybe<string[]>>
    selectInputFile(): Promise<Maybe<string>>
    selectInputFolder(): Promise<Maybe<SelectInputFolderResult>>
    selectOutputFolder(): Promise<Maybe<string>>
    selectOutputPath(defaultPath?: string): Promise<Maybe<string>>
    readImagePreview(filePath: string): Promise<Maybe<string>>
    convertImage(payload: ConvertImageRequest): Promise<ConvertImageResponse>
    cancelImage(): Promise<CancelResponse>
    convertPreview(payload: ConvertPreviewRequest): Promise<PreviewResult>
    convertPreviewData(payload: ConvertPreviewDataRequest): Promise<PreviewResult>
    batchConvertImages(payload: BatchConvertImagesRequest): Promise<BatchConvertResponse>
    batchCancelImages(payload: BatchCancelImagesRequest): Promise<CancelResponse>
    openOutputFolder(filePath: string): Promise<OpenOutputFolderResponse>
    getBackendStatus(): Promise<BackendStatusResponse>
    onConversionProgress(callback: (progress: ConversionProgressEvent) => void): Unsubscribe
    onBatchProgress(callback: (progress: BatchProgressEvent) => void): Unsubscribe

    // ---------- 视频 ----------
    selectInputVideo(): Promise<Maybe<string>>
    selectOutputVideo(defaultPath?: string): Promise<Maybe<string>>
    probeVideo(inputPath: string): Promise<ProbeVideoResult>
    readHdrMeta(inputPath: string): Promise<ReadHdrMetaResult>
    analyzeEclipsaWindows(payload: AnalyzeEclipsaWindowsRequest): Promise<AnalyzeEclipsaWindowsResult>
    extractVideoFirstFrame(inputPath: string): Promise<ExtractFrameResult>
    convertVideo(payload: ConvertVideoRequest): Promise<ConvertVideoResult>
    cancelVideo(): Promise<CancelResponse>
    onVideoProgress(callback: (progress: VideoProgressEvent) => void): Unsubscribe

    // ---------- 系统主题（MD3 动态取色） ----------
    /**
     * 系统主题（MD3 动态取色）—— main.js `get-system-theme`（**同步** sendSync）。
     *
     * ⚠️ 必须是同步的：`views/md3.js` L153 写的是 `var st = api.getSystemTheme()`（无 await），
     * 若改成 `ipcRenderer.invoke`（返回 Promise），`st.accentColor` 会是 undefined，
     * 于是静默回退到硬编码的 `#006FC4` —— 接线看起来接上了、实际没生效。
     */
    getSystemTheme(): SystemTheme
    /** 主题/主题色变化推送（`system-theme-changed`）；回调收到新的 SystemTheme */
    onSystemTheme(callback: (theme: SystemTheme) => void): Unsubscribe
  }

  /**
   * 系统主题快照 —— main.js `readSystemTheme()`。
   *
   * `accentColor` 的格式是契约里最容易错的一环：必须是 **'#RRGGBBAA'（9 字符，带 `#`）**。
   * `systemPreferences.getAccentColor()` 原生返回 8 字符 `'RRGGBBAA'`（**不带 `#`**），
   * main.js 负责补上 `#`；若漏补，`views/md3.js` 会因 `length !== 9` 跳过 alpha 裁剪，
   * 把 8 位串喂给要求 `slice(1,3)` 的 `hexToHsl()`，得到错误的调色板。
   */
  interface SystemTheme {
    /** '#RRGGBBAA'；平台不支持时为空字符串（md3.js 据此回退默认色） */
    accentColor: string
    /** 已由 nativeTheme 解析的确定值（不是 'system'） */
    theme: 'dark' | 'light'
  }

  // ==========================================================================
  //  8. window.HdrPreviewMedia（views/hdr_preview_media.js）
  // ==========================================================================

  /**
   * `init()` 的挂接参数。实现里逐项 `opts.x || null`，故全部可选。
   * video.html L1083-1092 传的是：renderCanvas / badgeEl / playBtn / seekEl /
   * timeEl / loopChk / frameScaleEl / mirrorVideo（**不传 nativeCanvas**）。
   */
  interface HdrPreviewInitOptions {
    nativeCanvas?: HTMLCanvasElement | null
    renderCanvas?: HTMLCanvasElement | null
    badgeEl?: HTMLElement | null
    playBtn?: HTMLElement | null
    seekEl?: HTMLInputElement | null
    timeEl?: HTMLElement | null
    loopChk?: HTMLInputElement | null
    frameScaleEl?: HTMLInputElement | null
    mirrorVideo?: HTMLVideoElement | null
  }

  /** `applyContent()` 的内容区参数。实现逐字段守卫，故全部可选（video.html 每次都传全 8 个） */
  interface HdrPreviewContentParams {
    tf?: TransferFunction
    gamut?: UiPrimaries | null
    /** 目标渲染亮度（显示峰值滑块） */
    dispPeak?: number
    /** 'pq'|'hlg' = 按所选输出传函呈现（Eclipsa 导出编码）；null = 不覆盖 */
    outTf?: VideoTransfer | null
    sdrGain?: number
    whiteNits?: number
    peakNits?: number
    isHdr?: boolean
  }

  /** `applyAgtm()` 的动态 2094-50 预览参数 */
  interface HdrPreviewAgtmParams {
    enabled: boolean
    windows?: EclipsaWindow[]
    gainSpace?: '2020' | 'p3' | 'srgb'
  }

  /** `hdr_preview_media.js` L1218-1311 `return {…}` 的完整公开面（12 个方法） */
  interface HdrPreviewMediaApi {
    init(opts: HdrPreviewInitOptions): void
    setMirrorVideo(el: HTMLVideoElement | null): void
    loadMedia(src: string): void
    applyContent(params: HdrPreviewContentParams): void
    getContentParams(): { tf: TransferFunction; gamut: UiPrimaries; maxNits: number }
    applyAgtm(params: HdrPreviewAgtmParams): void
    refreshFrame(): void
    play(): void
    pause(): void
    seekTo(seconds: number): void
    isPaused(): boolean
    destroy(): void
  }
}
