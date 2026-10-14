package io.github.hassanm0301.plasmaslate.video

import android.media.MediaCodec
import android.media.MediaFormat
import android.os.Build
import android.os.Handler
import android.os.HandlerThread
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
) {
    private class Frame(val data: ByteArray, val key: Boolean, val receivedNs: Long)

    private val thread = HandlerThread("slate-decoder").apply { start() }
    private val handler = Handler(thread.looper)
    private val lock = Any()
    private var codec: MediaCodec? = null
    private val freeInputs = ArrayDeque<Int>()
    private val pending = ArrayDeque<Frame>()
    private var waitingForKey = true
    private var released = false
    private var pts = 0L
    private val inFlight = HashMap<Long, Long>() // pts -> receivedNs

    // Statistics (since last snapshot)
    private var received = 0
    private var decoded = 0
    private var dropped = 0
    private var decodeNsSum = 0L
    @Volatile var lastDecodeMs: Float = 0f
        private set
    @Volatile var fps: Float = 0f
        private set
    private var fpsWindowStart = SystemClock.elapsedRealtime()
    private var fpsCount = 0
    val hevc: Boolean get() = mime == "video/hevc"

    /** Submits one access unit (Annex-B); called from the network thread. */
    fun submit(data: ByteArray, keyframe: Boolean, receivedNs: Long) {
        synchronized(lock) {
            if (released) return
            received++
            if (waitingForKey && !keyframe) {
                dropped++
                return
            }
            waitingForKey = false
            pending.addLast(Frame(data, keyframe, receivedNs))
            if (pending.size > MAX_PENDING) {
                // Decoder can't keep up: start over at the next keyframe.
                dropped += pending.size
                pending.clear()
                waitingForKey = true
                onKeyframeNeeded("decoder backlog")
                return
            }
        }
        handler.post { drain() }
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
        if (name != null && name.startsWith("OMX.hisi", ignoreCase = true)) {
            // HiSilicon (Kirin) low-latency decode mode.
            format.setInteger("vendor.hisi-ext-low-latency-video-dec.video-scene-for-low-latency-req", 1)
            format.setInteger("vendor.hisi-ext-low-latency-video-dec.video-scene-for-low-latency-rdy", -1)
        }
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
                    inFlight[pts] = frame.receivedNs
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
            try {
                codec.releaseOutputBuffer(index, info.size > 0)
            } catch (e: IllegalStateException) {
                return
            }
            val now = SystemClock.elapsedRealtimeNanos()
            synchronized(lock) {
                inFlight.remove(info.presentationTimeUs)?.let {
                    val d = now - it
                    decodeNsSum += d
                    lastDecodeMs = d / 1e6f
                }
                // Forget stale entries (frames the decoder dropped).
                if (inFlight.size > 32) inFlight.keys.filter { it < info.presentationTimeUs }.forEach { inFlight.remove(it) }
                decoded++
                fpsCount++
                val t = SystemClock.elapsedRealtime()
                if (t - fpsWindowStart >= 1000) {
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
        received = 0; decoded = 0; dropped = 0; decodeNsSum = 0
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
    }
}
