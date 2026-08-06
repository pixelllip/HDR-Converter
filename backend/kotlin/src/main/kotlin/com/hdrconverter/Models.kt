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
    @SerialName("quality") val quality: Double = 1.0
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
    @SerialName("message") val message: String? = null
)

/**
 * 预览请求
 */
@Serializable
data class PreviewRequest(
    @SerialName("inputPath") val inputPath: String,
    @SerialName("settings") val settings: ConversionSettings? = null
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
