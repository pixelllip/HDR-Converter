package com.hdrconverter

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable

/**
 * HDR 转换参数设置
 */
@Serializable
data class ConversionSettings(
    @SerialName("hdrIntensity") val hdrIntensity: Double = 1.18,
    @SerialName("fineTuneBrightness") val fineTuneBrightness: Double = 0.3,
    @SerialName("gamma") val gamma: Double = 0.9,
    @SerialName("rgbAdjustment") val rgbAdjustment: RgbAdjustment? = null,
    @SerialName("outputFormat") val outputFormat: String = "png",
    /** JPEG 质量 0..1（默认 1.0 = 100%），仅 jpg / jpg_icc 输出生效 */
    @SerialName("quality") val quality: Double = 1.0,
    /**
     * 实验：true 时主图（SDR 底图）用原始 sRGB 像素 + sRGB ICC（任何查看器看到原图、保色调）；
     * false（默认）保持 Display-P3 像素 + P3 ICC（与真实 Google 文件一致，需查看器色彩管理）。
     */
    @SerialName("primarySrgb") val primarySrgb: Boolean = false,
    /** 白点（SDR 参考白，尼特，默认 203）与峰值亮度（高光上限/增益图 maxBoost 上限，默认 1000） */
    @SerialName("whiteNits") val whiteNits: Double? = null,
    @SerialName("peakNits") val peakNits: Double? = null
) {
    val totalExposure: Double get() = hdrIntensity * fineTuneBrightness
    val rAdj: Double get() = rgbAdjustment?.red ?: 0.96
    val gAdj: Double get() = rgbAdjustment?.green ?: 1.0
    val bAdj: Double get() = rgbAdjustment?.blue ?: 1.0
}

@Serializable
data class RgbAdjustment(
    @SerialName("red") val red: Double = 0.96,
    @SerialName("green") val green: Double = 1.0,
    @SerialName("blue") val blue: Double = 1.0
)

/**
 * 转换请求
 */
@Serializable
data class ConvertRequest(
    @SerialName("inputPath") val inputPath: String,
    @SerialName("outputPath") val outputPath: String? = null,
    @SerialName("settings") val settings: ConversionSettings? = null
)

@Serializable
data class ConvertResponse(
    @SerialName("success") val success: Boolean,
    @SerialName("outputPath") val outputPath: String? = null,
    @SerialName("outputFormat") val outputFormat: String? = null,
    @SerialName("message") val message: String? = null,
    /** 检测到的输入图像色彩空间（先检测原图色彩空间再转换） */
    @SerialName("detectedColorSpace") val detectedColorSpace: String? = null
)

/**
 * 预览请求
 */
@Serializable
data class PreviewRequest(
    @SerialName("inputPath") val inputPath: String,
    @SerialName("settings") val settings: ConversionSettings? = null,
    /** 预览模式：null=图片 jpg_icc/png/Ultra HDR；videoDirect=视频直接转（Rec.2020/PQ 与视频输出一致） */
    @SerialName("mode") val mode: String? = null
)

@Serializable
data class PreviewResponse(
    @SerialName("dataUrl") val dataUrl: String? = null,
    @SerialName("width") val width: Int = 0,
    @SerialName("height") val height: Int = 0,
    @SerialName("aspectRatio") val aspectRatio: Double = 0.0
)

/**
 * 进度推送
 */
@Serializable
data class ProgressMessage(
    @SerialName("value") val value: Double,
    @SerialName("message") val message: String,
    @SerialName("jobId") val jobId: String? = null
)

/**
 * 批量转换请求
 */
@Serializable
data class BatchJob(
    @SerialName("inputPath") val inputPath: String,
    @SerialName("outputPath") val outputPath: String? = null,
    @SerialName("settings") val settings: ConversionSettings? = null
)

@Serializable
data class BatchConvertRequest(
    @SerialName("jobs") val jobs: List<BatchJob> = emptyList(),
    @SerialName("maxConcurrent") val maxConcurrent: Int? = null
)

@Serializable
data class BatchJobResult(
    @SerialName("inputPath") val inputPath: String,
    @SerialName("outputPath") val outputPath: String? = null,
    @SerialName("success") val success: Boolean,
    @SerialName("message") val message: String? = null
)

@Serializable
data class BatchConvertResponse(
    @SerialName("results") val results: List<BatchJobResult>,
    @SerialName("successCount") val successCount: Int,
    @SerialName("failCount") val failCount: Int
)

/**
 * 批量取消请求
 */
@Serializable
data class BatchCancelRequest(
    @SerialName("inputPaths") val inputPaths: List<String> = emptyList()
)

/**
 * 批量进度（含逐项状态：inputPath -> queued|running|done|failed|cancelled）
 */
@Serializable
data class BatchProgressResponse(
    @SerialName("total") val total: Int = 0,
    @SerialName("done") val done: Int = 0,
    @SerialName("failed") val failed: Int = 0,
    @SerialName("current") val current: String = "",
    @SerialName("message") val message: String = "",
    @SerialName("running") val running: Boolean = false,
    @SerialName("statuses") val statuses: Map<String, String> = emptyMap()
)

/**
 * 自动估算 HDR 强度请求
 */
@Serializable
data class EstimateRequest(
    @SerialName("inputPath") val inputPath: String
)

/**
 * 自动估算 HDR 强度响应
 */
@Serializable
data class EstimateResponse(
    @SerialName("hdrIntensity") val hdrIntensity: Double,
    @SerialName("maxBoost") val maxBoost: Double,
    @SerialName("yP995") val yP995: Double,
    @SerialName("hlRatio") val hlRatio: Double,
    @SerialName("message") val message: String = ""
)

/**
 * 视频逐帧重建请求（链路 2）
 * inputPath 指向一张临时 SDR 帧图片（PNG/JPEG）
 */
@Serializable
data class VideoFrameRequest(
    @SerialName("inputPath") val inputPath: String,
    @SerialName("settings") val settings: ConversionSettings? = null,
    /** 输出归一化峰值（默认 8.0；编码 npl = 100*peak 尼特） */
    @SerialName("peak") val peak: Double? = null,
    /** 重建模式：gainmap=增益图（默认）| transform=图片 ICC 增益式单层 */
    @SerialName("mode") val mode: String? = null
)

/**
 * 视频逐帧重建响应：16-bit PAM（大端 RGB，sRGB 线性，已归一化到 peak）
 */
@Serializable
data class VideoFrameResponse(
    @SerialName("pamBase64") val pamBase64: String,
    @SerialName("width") val width: Int,
    @SerialName("height") val height: Int
)
