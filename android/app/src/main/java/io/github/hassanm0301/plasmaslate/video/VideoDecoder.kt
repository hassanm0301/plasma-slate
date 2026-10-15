package io.github.hassanm0301.plasmaslate.video

import android.media.MediaCodec
import android.media.MediaFormat
import android.os.Build
import android.os.Handler
import android.os.HandlerThread
import android.os.Process
import android.os.SystemClock
import android.util.Log
import android.view.Surface
import io.github.hassanm0301.plasmaslate.proto.ClientStats
import java.nio.ByteBuffer

/**
 * Low-latency hardware decoder rendering straight to a [Surface].
 *
 * The codec is configured lazily from the first keyframe (its SPS/PPS become csd-0/csd-1),
 * frames are queued as soon as an input buffer is free and output buffers are released for
 * rendering immediately.
 */
class VideoDecoder(
    private val surface: Surface,
    private val mime: String,
    private val width: Int,
    private val height: Int,
    private val codecName: String?,
    private val onKeyframeNeeded: (String) -> Unit,
    /** server clock − client clock (µs) for capture-to-display latency, null if unknown. */
    private val clockOffsetUs: () -> Long? = { null },
    private val vendorLowLatency: Boolean = false,
    /** Append an access-unit delimiter to each frame (see [NalUnits.withTrailingAud]). */
    private val trailingAud: Boolean = false,
) {
    private class Frame(val data: ByteArray, val key: Boolean, val filler: Boolean, val receivedNs: Long, val captureUs: Long)
    private class InFlight(val receivedNs: Long, val captureUs: Long, val filler: Boolean)

    // Callbacks run on this thread with display priority: on a small SoC a late output
    // callback shows up directly as latency.
    private val thread = HandlerThread("slate-decoder", Process.THREAD_PRIORITY_URGENT_DISPLAY).apply { start() }
    private val handler = Handler(thread.looper)
    private val lock = Any()
    private var codec: MediaCodec? = null
    private val freeInputs = ArrayDeque<Int>()
    private val pending = ArrayDeque<Frame>()
    private var waitingForKey = true
    private var released = false
    private var pts = 0L
    private val inFlight = HashMap<Long, InFlight>() // pts -> timing

    // Statistics (since last snapshot)
    private var received = 0
    private var decoded = 0
    private var dropped = 0
    private var decodeNsSum = 0L
    private var latencyUsSum = 0L
    private var latencyCount = 0
    @Volatile var lastLatencyMs: Float = 0f
        private set
    @Volatile var lastDecodeMs: Float = 0f
        private set
    @Volatile var fps: Float = 0f
        private set
    private var fpsWindowStart = SystemClock.elapsedRealtime()
    // Backlog control: a decoder that is only just fast enough never drains a backlog, so when
    // frames keep waiting (or decode latency stays far above its floor) we flush and resync.
    private var backlogSinceMs = 0L
    private var lastFlushMs = 0L
    private var decodeFloorMs = Float.MAX_VALUE
    private var slowSinceMs = 0L
    @Volatile var flushes = 0
        private set
    private var fpsCount = 0
    val hevc: Boolean get() = mime == "video/hevc"

    /**
     * Submits one access unit (Annex-B); called from the network thread, which also feeds the
     * codec directly (no thread hop). [filler] frames are decoded but not shown.
     */
    fun submit(data: ByteArray, keyframe: Boolean, receivedNs: Long, captureUs: Long = 0, filler: Boolean = false) {
        synchronized(lock) {
            if (released) return
            received++
            if (waitingForKey && !keyframe) {
                dropped++
                return
            }
            waitingForKey = false
            pending.addLast(Frame(if (trailingAud) NalUnits.withTrailingAud(data, hevc) else data, keyframe, filler, receivedNs, captureUs))
            val nowMs = SystemClock.elapsedRealtime()
            if (pending.size >= BACKLOG_FRAMES) {
                if (backlogSinceMs == 0L) backlogSinceMs = nowMs
            } else {
                backlogSinceMs = 0L
            }
            val stuck = backlogSinceMs != 0L && nowMs - backlogSinceMs > BACKLOG_MS
            if (stuck && nowMs - lastFlushMs > MIN_FLUSH_INTERVAL_MS) {
                lastFlushMs = nowMs
                backlogSinceMs = 0L
                handler.post { flushAndResync("input backlog") }
                return
            }
            if (pending.size > MAX_PENDING) {
                // Decoder can't keep up: start over at the next keyframe.
                dropped += pending.size
                pending.clear()
                waitingForKey = true
                onKeyframeNeeded("decoder backlog")
                return
            }
        }
        drain()
    }

    private fun ensureCodec(first: Frame): MediaCodec? {
        codec?.let { return it }
        val (config, _) = NalUnits.splitConfig(first.data, hevc)
        val format = MediaFormat.createVideoFormat(mime, width, height)
        if (config.isNotEmpty() && !hevc) {
            // H.264: csd-0 = SPS, csd-1 = PPS (each with start code).
            val starts = NalUnits.nalStarts(config)
            if (starts.size >= 2) {
                val second = starts[1] - (if (starts[1] >= 4 && config[starts[1] - 4] == 0.toByte()) 4 else 3)
                format.setByteBuffer("csd-0", ByteBuffer.wrap(config.copyOfRange(0, second)))
                format.setByteBuffer("csd-1", ByteBuffer.wrap(config.copyOfRange(second, config.size)))
            }
        } else if (config.isNotEmpty()) {
            format.setByteBuffer("csd-0", ByteBuffer.wrap(config))
        }
        format.setInteger(MediaFormat.KEY_PRIORITY, 0) // realtime
        format.setInteger(MediaFormat.KEY_OPERATING_RATE, Short.MAX_VALUE.toInt())
        if (Build.VERSION.SDK_INT >= 30) format.setInteger(MediaFormat.KEY_LOW_LATENCY, 1)
        val name = codecName
        if (vendorLowLatency && name != null) applyVendorLowLatency(format, name)
        val c = try {
            if (name != null) MediaCodec.createByCodecName(name) else MediaCodec.createDecoderByType(mime)
        } catch (e: Exception) {
            Log.e(TAG, "cannot create decoder", e)
            return null
        }
        c.setCallback(callback, handler)
        try {
            c.configure(format, surface, null, 0)
            c.start()
        } catch (e: Exception) {
            Log.e(TAG, "cannot configure decoder ${c.name}", e)
            c.release()
            return null
        }
        Log.i(TAG, "decoder ${c.name} started ${width}x$height")
        codec = c
        return c
    }

    /**
     * Pre-Android 11 style low-latency switches, named per vendor. Unknown keys are ignored by
     * the codec, so only the matching family's keys are set.
     */
    private fun applyVendorLowLatency(format: MediaFormat, name: String) {
        val n = name.lowercase()
        when {
            n.startsWith("omx.hisi") || n.startsWith("c2.hisi") -> {
                // HiSilicon (Kirin). Hurts on some firmware: see docs/spikes/s4-android-decode.md.
                format.setInteger("vendor.hisi-ext-low-latency-video-dec.video-scene-for-low-latency-req", 1)
                format.setInteger("vendor.hisi-ext-low-latency-video-dec.video-scene-for-low-latency-rdy", -1)
            }
            n.startsWith("omx.qcom") || n.startsWith("c2.qti") -> {
                format.setInteger("vendor.qti-ext-dec-low-latency.enable", 1)
                format.setInteger("vendor.rtc-ext-dec-low-latency.enable", 1)
            }
            n.startsWith("omx.exynos") || n.startsWith("c2.exynos") || n.startsWith("c2.sec") ->
                format.setInteger("vendor.rtc-ext-dec-low-latency.enable", 1)
            n.startsWith("omx.amlogic") || n.startsWith("c2.amlogic") ->
                format.setInteger("vendor.low-latency.enable", 1)
            n.startsWith("omx.mtk") || n.startsWith("c2.mtk") ->
                format.setInteger("vendor.mediatek.ext.low-latency", 1)
        }
    }

    private fun drain() {
        while (true) {
            val frame: Frame
            val index: Int
            val c: MediaCodec
            synchronized(lock) {
                if (released) return
                val head = pending.firstOrNull() ?: return
                val cc = codec ?: ensureCodec(head) ?: run {
                    pending.clear(); return
                }
                c = cc
                index = freeInputs.removeFirstOrNull() ?: return
                frame = pending.removeFirst()
            }
            try {
                val buf = c.getInputBuffer(index) ?: continue
                buf.clear()
                if (frame.data.size > buf.capacity()) {
                    Log.w(TAG, "frame ${frame.data.size} B > input buffer ${buf.capacity()} B")
                    c.queueInputBuffer(index, 0, 0, 0, 0)
                    onKeyframeNeeded("frame too large")
                    continue
                }
                buf.put(frame.data)
                val p = synchronized(lock) {
                    pts += 16_666
                    inFlight[pts] = InFlight(frame.receivedNs, frame.captureUs, frame.filler)
                    pts
                }
                c.queueInputBuffer(index, 0, frame.data.size, p, 0)
            } catch (e: IllegalStateException) {
                Log.w(TAG, "queueInputBuffer failed", e)
                reset("decoder error")
                return
            }
        }
    }

    private val callback = object : MediaCodec.Callback() {
        override fun onInputBufferAvailable(codec: MediaCodec, index: Int) {
            synchronized(lock) { freeInputs.addLast(index) }
            drain()
        }

        override fun onOutputBufferAvailable(codec: MediaCodec, index: Int, info: MediaCodec.BufferInfo) {
            // Fillers repeat the picture already on screen: decoding them is the point, showing
            // them would only queue identical buffers for the compositor.
            val filler = synchronized(lock) { inFlight[info.presentationTimeUs]?.filler == true }
            try {
                codec.releaseOutputBuffer(index, info.size > 0 && !filler)
            } catch (e: IllegalStateException) {
                return
            }
            val now = SystemClock.elapsedRealtimeNanos()
            synchronized(lock) {
                inFlight.remove(info.presentationTimeUs)?.takeIf { !it.filler }?.let {
                    val d = now - it.receivedNs
                    decodeNsSum += d
                    lastDecodeMs = d / 1e6f
                    if (lastDecodeMs < decodeFloorMs) decodeFloorMs = lastDecodeMs
                    val nowMs = now / 1_000_000
                    if (lastDecodeMs > SLOW_DECODE_MS && lastDecodeMs > decodeFloorMs * 2.5f) {
                        if (slowSinceMs == 0L) slowSinceMs = nowMs
                        if (nowMs - slowSinceMs > BACKLOG_MS && nowMs - lastFlushMs > MIN_FLUSH_INTERVAL_MS) {
                            lastFlushMs = nowMs
                            slowSinceMs = 0L
                            handler.post { flushAndResync("decoder latency ${lastDecodeMs.toInt()} ms") }
                        }
                    } else {
                        slowSinceMs = 0L
                    }
                    val off = clockOffsetUs()
                    if (off != null && it.captureUs > 0) {
                        val lat = now / 1000 + off - it.captureUs
                        if (lat in 0..5_000_000) {
                            latencyUsSum += lat
                            latencyCount++
                            lastLatencyMs = lat / 1000f
                        }
                    }
                }
                // Forget stale entries (frames the decoder dropped).
                if (inFlight.size > 32) inFlight.keys.filter { it < info.presentationTimeUs }.forEach { inFlight.remove(it) }
                decoded++
                if (filler) return
                fpsCount++
                val t = SystemClock.elapsedRealtime()
                if (t - fpsWindowStart >= 1000) {
                    Log.i(TAG, "perf fps=$fpsCount decode_ms=$lastDecodeMs latency_ms=$lastLatencyMs pending=${pending.size} flushes=$flushes codec=$mime decoder=${codec.name} vendorLL=$vendorLowLatency aud=$trailingAud")
                    fps = fpsCount * 1000f / (t - fpsWindowStart)
                    fpsCount = 0
                    fpsWindowStart = t
                }
            }
        }

        override fun onError(codec: MediaCodec, e: MediaCodec.CodecException) {
            Log.w(TAG, "decoder error recoverable=${e.isRecoverable} transient=${e.isTransient}", e)
            if (!e.isTransient) reset("decoder error ${e.diagnosticInfo}")
        }

        override fun onOutputFormatChanged(codec: MediaCodec, format: MediaFormat) {
            Log.i(TAG, "output format $format")
        }
    }

    /** Drops everything queued inside and in front of the decoder; resumes at the next keyframe. */
    private fun flushAndResync(reason: String) {
        val c: MediaCodec
        synchronized(lock) {
            c = codec ?: return
            dropped += pending.size
            pending.clear()
            freeInputs.clear()
            inFlight.clear()
            waitingForKey = true
            flushes++
        }
        try {
            c.flush()
            c.start() // async mode: input callbacks resume after start()
        } catch (e: IllegalStateException) {
            reset("flush failed")
            return
        }
        Log.i(TAG, "flushed decoder ($reason)")
        onKeyframeNeeded(reason)
    }

    /** Throws the codec away and waits for the next keyframe. */
    private fun reset(reason: String) {
        val old: MediaCodec?
        synchronized(lock) {
            old = codec
            codec = null
            freeInputs.clear()
            pending.clear()
            inFlight.clear()
            waitingForKey = true
        }
        try { old?.stop() } catch (_: Exception) {}
        try { old?.release() } catch (_: Exception) {}
        onKeyframeNeeded(reason)
    }

    fun stats(): ClientStats.Builder = synchronized(lock) {
        val b = ClientStats.newBuilder()
            .setFramesReceived(received)
            .setFramesDecoded(decoded)
            .setFramesDropped(dropped)
            .setAvgDecodeUs(if (decoded > 0) (decodeNsSum / decoded / 1000).toInt() else 0)
            .setAvgLatencyUs(if (latencyCount > 0) (latencyUsSum / latencyCount).toInt() else 0)
        received = 0; decoded = 0; dropped = 0; decodeNsSum = 0; latencyUsSum = 0; latencyCount = 0
        b
    }

    fun release() {
        val c: MediaCodec?
        synchronized(lock) {
            released = true
            c = codec
            codec = null
        }
        handler.post {
            try { c?.stop() } catch (_: Exception) {}
            try { c?.release() } catch (_: Exception) {}
            thread.quitSafely()
        }
    }

    companion object {
        private const val TAG = "VideoDecoder"
        private const val MAX_PENDING = 8
        private const val BACKLOG_FRAMES = 3
        private const val BACKLOG_MS = 400L
        private const val SLOW_DECODE_MS = 70f
        private const val MIN_FLUSH_INTERVAL_MS = 3000L
    }
}
