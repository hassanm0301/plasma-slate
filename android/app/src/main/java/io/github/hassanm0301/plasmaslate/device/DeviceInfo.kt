package io.github.hassanm0301.plasmaslate.device

import android.app.Activity
import android.media.MediaCodecInfo
import android.media.MediaCodecList
import android.os.Build
import android.util.DisplayMetrics
import io.github.hassanm0301.plasmaslate.proto.Codec
import io.github.hassanm0301.plasmaslate.proto.DecoderInfo
import io.github.hassanm0301.plasmaslate.proto.DisplayInfo
import io.github.hassanm0301.plasmaslate.proto.Orientation
import io.github.hassanm0301.plasmaslate.proto.TouchInfo
import kotlin.math.max
import kotlin.math.min

/** Facts about this tablet that the server needs to build a matching screen. */
object DeviceInfo {
    const val MIME_H264 = "video/avc"
    const val MIME_HEVC = "video/hevc"

    fun deviceName(): String {
        val m = Build.MODEL ?: "Android"
        val b = Build.MANUFACTURER ?: ""
        return if (m.startsWith(b, ignoreCase = true)) m else "$b $m".trim()
    }

    /** Panel size in landscape (long side first), physical size and density. */
    fun display(activity: Activity): DisplayInfo {
        @Suppress("DEPRECATION")
        val display = activity.windowManager.defaultDisplay
        val dm = DisplayMetrics()
        @Suppress("DEPRECATION")
        display.getRealMetrics(dm)
        val w = max(dm.widthPixels, dm.heightPixels)
        val h = min(dm.widthPixels, dm.heightPixels)
        // xdpi/ydpi follow the current orientation; use the average for robustness.
        val dpi = ((dm.xdpi + dm.ydpi) / 2f).takeIf { it > 50f } ?: dm.densityDpi.toFloat()
        return DisplayInfo.newBuilder()
            .setWidthPx(w)
            .setHeightPx(h)
            .setPhysicalWidthMm(w / dpi * 25.4f)
            .setPhysicalHeightMm(h / dpi * 25.4f)
            .setDensityDpi(dpi.toInt())
            .setRefreshHz(display.refreshRate)
            .setOrientation(Orientation.ORIENTATION_LANDSCAPE)
            .build()
    }

    fun touch(activity: Activity): TouchInfo {
        val pm = activity.packageManager
        val multi = pm.hasSystemFeature("android.hardware.touchscreen.multitouch.jazzhand")
        val any = pm.hasSystemFeature("android.hardware.touchscreen")
        return TouchInfo.newBuilder().setSupported(any).setMaxPoints(if (multi) 10 else if (any) 2 else 0).build()
    }

    /** Best decoder per codec (hardware preferred). */
    fun decoders(): List<DecoderInfo> {
        val infos = MediaCodecList(MediaCodecList.REGULAR_CODECS).codecInfos.filter { !it.isEncoder }
        return listOf(MIME_H264 to Codec.CODEC_H264, MIME_HEVC to Codec.CODEC_HEVC).mapNotNull { (mime, codec) ->
            val candidates = infos.filter { info -> info.supportedTypes.any { it.equals(mime, ignoreCase = true) } }
            val best = candidates.firstOrNull { it.isHardwareAccelerated } ?: candidates.firstOrNull() ?: return@mapNotNull null
            val caps = best.getCapabilitiesForType(mime).videoCapabilities
            DecoderInfo.newBuilder()
                .setCodec(codec)
                .setMaxWidth(caps?.supportedWidths?.upper ?: 1920)
                .setMaxHeight(caps?.supportedHeights?.upper ?: 1088)
                .setMaxFps(60)
                .setHardware(best.isHardwareAccelerated)
                .setName(best.name)
                .build()
        }
    }

    fun mimeFor(codec: Codec): String = if (codec == Codec.CODEC_HEVC) MIME_HEVC else MIME_H264

    /** Name of the preferred decoder for [mime] (hardware first), or null for the platform default. */
    fun decoderName(mime: String): String? {
        val infos: List<MediaCodecInfo> = MediaCodecList(MediaCodecList.REGULAR_CODECS).codecInfos.filter { !it.isEncoder }
        val c = infos.filter { info -> info.supportedTypes.any { it.equals(mime, ignoreCase = true) } }
        return (c.firstOrNull { it.isHardwareAccelerated } ?: c.firstOrNull())?.name
    }
}
