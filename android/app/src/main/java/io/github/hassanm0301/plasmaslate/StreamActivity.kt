package io.github.hassanm0301.plasmaslate

import android.annotation.SuppressLint
import android.app.Activity
import android.content.Context
import android.graphics.Color
import android.net.wifi.WifiManager
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.util.TypedValue
import android.view.Gravity
import android.view.MotionEvent
import android.view.SurfaceHolder
import android.view.SurfaceView
import android.view.View
import android.view.WindowInsets
import android.view.WindowInsetsController
import android.view.WindowManager
import android.widget.FrameLayout
import android.widget.TextView
import android.widget.Toast
import android.window.OnBackInvokedDispatcher
import io.github.hassanm0301.plasmaslate.data.Endpoint
import io.github.hassanm0301.plasmaslate.data.Prefs
import io.github.hassanm0301.plasmaslate.device.DeviceInfo
import io.github.hassanm0301.plasmaslate.input.TouchMapper
import io.github.hassanm0301.plasmaslate.net.PinningTrustManager
import io.github.hassanm0301.plasmaslate.net.SlateConnection
import io.github.hassanm0301.plasmaslate.proto.ClientHello
import io.github.hassanm0301.plasmaslate.proto.ClientStats
import io.github.hassanm0301.plasmaslate.proto.ErrorCode
import io.github.hassanm0301.plasmaslate.proto.ServerHello
import io.github.hassanm0301.plasmaslate.proto.VideoFrame
import io.github.hassanm0301.plasmaslate.video.VideoDecoder

/** Full-screen display of one PC session; forwards touch; reconnects automatically. */
class StreamActivity : Activity(), SurfaceHolder.Callback, SlateConnection.Listener {
    private val main = Handler(Looper.getMainLooper())
    private lateinit var prefs: Prefs
    private lateinit var endpoint: Endpoint
    private lateinit var surfaceView: SurfaceView
    private lateinit var status: TextView
    private lateinit var stats: TextView
    private var surfaceReady = false
    private var connection: SlateConnection? = null
    @Volatile private var decoder: VideoDecoder? = null
    private var mapper: TouchMapper? = null
    private var stream: ServerHello? = null
    private var wifiLock: WifiManager.WifiLock? = null
    private var retryDelayS = 1
    private var stopped = true
    /** Developer options from the launch intent (`--es debug_codec hevc`, `--ez debug_vendor_low_latency false`). */
    private val debugCodec by lazy {
        when (intent.getStringExtra("debug_codec")?.lowercase()) {
            "hevc", "h265" -> io.github.hassanm0301.plasmaslate.proto.Codec.CODEC_HEVC
            "h264", "avc" -> io.github.hassanm0301.plasmaslate.proto.Codec.CODEC_H264
            else -> null
        }
    }
    /** Latency options: developer extras override the saved preferences. */
    private val decoderMode by lazy {
        Prefs.DecoderMode.entries.firstOrNull { it.key == intent.getStringExtra("debug_decoder") } ?: prefs.decoderMode
    }
    private val vendorLowLatency: Boolean by lazy {
        when {
            intent.hasExtra("debug_vendor_low_latency") -> intent.getBooleanExtra("debug_vendor_low_latency", false)
            else -> when (prefs.vendorLowLatency) {
                Prefs.Tristate.ON -> true
                Prefs.Tristate.OFF -> false
                // Measured to hurt on HiSilicon (docs/spikes/s4-android-decode.md); helps elsewhere.
                Prefs.Tristate.AUTO -> Build.HARDWARE?.lowercase()?.let { !it.startsWith("kirin") && !it.startsWith("hi3") } ?: true
            }
        }
    }
    private val trailingAud by lazy {
        if (intent.hasExtra("debug_trailing_aud")) intent.getBooleanExtra("debug_trailing_aud", false) else prefs.trailingAud
    }

    @SuppressLint("ClickableViewAccessibility")
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        prefs = Prefs(this)
        endpoint = Endpoint(
            intent.getStringExtra(EXTRA_HOST) ?: "127.0.0.1",
            intent.getIntExtra(EXTRA_PORT, Prefs.DEFAULT_PORT),
            intent.getStringExtra(EXTRA_NAME) ?: "PC",
            intent.getBooleanExtra(EXTRA_USB, false),
        )
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        if (prefs.maxRefreshRate) preferHighestRefreshRate()
        surfaceView = SurfaceView(this)
        surfaceView.holder.addCallback(this)
        status = TextView(this).apply {
            setTextColor(Color.WHITE)
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 20f)
            gravity = Gravity.CENTER
            setBackgroundColor(0xCC000000.toInt())
            setPadding(48, 32, 48, 32)
        }
        stats = TextView(this).apply {
            setTextColor(Color.WHITE)
            setBackgroundColor(0x88000000.toInt())
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 12f)
            setPadding(12, 6, 12, 6)
            visibility = View.GONE
        }
        val root = FrameLayout(this)
        root.addView(surfaceView, FrameLayout.LayoutParams(-1, -1))
        root.addView(status, FrameLayout.LayoutParams(-2, -2, Gravity.CENTER))
        root.addView(stats, FrameLayout.LayoutParams(-2, -2, Gravity.TOP or Gravity.START))
        setContentView(root)
        surfaceView.setOnTouchListener { _, ev -> onSurfaceTouch(ev) }
        hideSystemUi()
        registerBack()
    }

    /**
     * Asks for the panel's fastest mode at its current resolution. The stream follows the
     * refresh rate reported in DisplayInfo, and hardware decoders that hold N frames hold them
     * for N frame intervals, so a 90/120 Hz panel cuts that wait by a third or a half.
     */
    private fun preferHighestRefreshRate() {
        @Suppress("DEPRECATION")
        val display = windowManager.defaultDisplay
        val current = display.mode
        val best = display.supportedModes
            .filter { it.physicalWidth == current.physicalWidth && it.physicalHeight == current.physicalHeight }
            .maxByOrNull { it.refreshRate } ?: return
        if (best.modeId != current.modeId) {
            window.attributes = window.attributes.apply { preferredDisplayModeId = best.modeId }
        }
    }

    override fun onResume() {
        super.onResume()
        hideSystemUi()
        val wm = applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
        wifiLock = wm.createWifiLock(WifiManager.WIFI_MODE_FULL_LOW_LATENCY, "plasma-slate").apply {
            setReferenceCounted(false)
            acquire()
        }
        stopped = false
        maybeConnect()
        main.post(statsTick)
    }

    override fun onPause() {
        super.onPause()
        stopped = true
        main.removeCallbacksAndMessages(null)
        disconnect("tablet app paused")
        wifiLock?.release()
        wifiLock = null
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (hasFocus) hideSystemUi()
    }

    private fun hideSystemUi() {
        if (Build.VERSION.SDK_INT >= 30) {
            window.insetsController?.let {
                it.hide(WindowInsets.Type.statusBars() or WindowInsets.Type.navigationBars())
                it.systemBarsBehavior = WindowInsetsController.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
            }
        } else {
            @Suppress("DEPRECATION")
            window.decorView.systemUiVisibility = (View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY or View.SYSTEM_UI_FLAG_FULLSCREEN or
                View.SYSTEM_UI_FLAG_HIDE_NAVIGATION or View.SYSTEM_UI_FLAG_LAYOUT_STABLE or
                View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION or View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN)
        }
    }

    // --- surface

    override fun surfaceCreated(holder: SurfaceHolder) {
        surfaceReady = true
        val conn = connection
        val hello = stream
        if (conn != null && hello != null) {
            // Surface recreated during a session (window relayout): only rebuild the decoder.
            createDecoder(conn, hello)
            conn.requestKeyframe("surface recreated")
        } else {
            maybeConnect()
        }
    }

    override fun surfaceChanged(holder: SurfaceHolder, format: Int, width: Int, height: Int) {
        stream?.stream?.let { mapper = TouchMapper(width, height, it.width, it.height) }
    }

    override fun surfaceDestroyed(holder: SurfaceHolder) {
        // Keep the session: the surface is often recreated right away (e.g. when the window is
        // relaid out at launch). onPause() is what ends the session.
        surfaceReady = false
        decoder?.release()
        decoder = null
    }

    // --- connection lifecycle

    private fun maybeConnect() {
        if (stopped || !surfaceReady || connection != null) return
        showStatus(getString(R.string.status_connecting, endpoint.name))
        val hello = ClientHello.newBuilder()
            .setDeviceId(prefs.deviceId)
            .setDeviceName(DeviceInfo.deviceName())
            .setAppVersion(BuildInfo.VERSION)
            .setDisplay(DeviceInfo.display(this))
            .setTouch(DeviceInfo.touch(this))
            .addAllDecoders(DeviceInfo.decoders(decoderMode).filter { d -> debugCodec?.let { it == d.codec } ?: true })
        connection = SlateConnection(endpoint, prefs, hello, this).also { it.start() }
    }

    private fun disconnect(reason: String) {
        connection?.stop(reason)
        connection = null
        decoder?.release()
        decoder = null
        stream = null
    }

    override fun onPairingPending(fingerprintShort: String) {
        main.post { showStatus(getString(R.string.status_pairing, fingerprintShort)) }
    }

    override fun onSessionStarted(hello: ServerHello) {
        main.post {
            val conn = connection ?: return@post
            val cfg = hello.stream
            stream = hello
            retryDelayS = 1
            showStatus(getString(R.string.status_waiting_video))
            if (surfaceReady) createDecoder(conn, hello)
            mapper = TouchMapper(surfaceView.width, surfaceView.height, cfg.width, cfg.height)
            conn.sendReady()
        }
    }

    private fun createDecoder(conn: SlateConnection, hello: ServerHello) {
        val cfg = hello.stream
        val mime = DeviceInfo.mimeFor(cfg.codec)
        decoder?.release()
        decoder = VideoDecoder(
            surfaceView.holder.surface, mime, cfg.width, cfg.height, DeviceInfo.decoderName(mime, decoderMode),
            onKeyframeNeeded = { reason -> connection?.requestKeyframe(reason) },
            clockOffsetUs = { conn.clockOffsetUs },
            vendorLowLatency = vendorLowLatency,
            trailingAud = trailingAud,
        ).also { dec ->
            conn.statsSource = object : SlateConnection.StatsSource {
                override fun snapshot(): ClientStats.Builder = dec.stats()
            }
        }
    }

    override fun onVideoFrame(frame: VideoFrame, receivedNs: Long) {
        val d = decoder ?: return
        d.submit(frame.data.toByteArray(), frame.keyframe, receivedNs, frame.captureTimeUs, frame.filler)
        if (frame.sequence == 0L) main.post {
            hideStatus()
            Toast.makeText(this, R.string.disconnect_hint, Toast.LENGTH_SHORT).show()
        }
    }

    override fun onEnded(reason: String, fatal: Boolean, code: ErrorCode?, reconnectNow: Boolean) {
        main.post {
            connection = null
            decoder?.release()
            decoder = null
            stream = null
            if (stopped) return@post
            when {
                reason == "identity-changed" -> showStatus(getString(R.string.status_identity_changed))
                code == ErrorCode.ERROR_CODE_PAIRING_DENIED -> showStatus(getString(R.string.status_denied))
                reconnectNow -> {
                    retryDelayS = 1
                    showStatus(getString(R.string.status_applying_settings))
                    main.postDelayed({ maybeConnect() }, 300)
                }
                fatal -> {
                    showStatus(getString(R.string.status_ended, reason))
                    main.postDelayed({ if (!stopped) finish() }, 2500)
                }
                else -> {
                    val delay = retryDelayS
                    retryDelayS = (retryDelayS * 2).coerceAtMost(8)
                    showStatus(getString(R.string.status_reconnecting, delay) + "\n" + reason)
                    main.postDelayed({ maybeConnect() }, delay * 1000L)
                }
            }
        }
    }

    // --- touch

    private fun onSurfaceTouch(ev: MotionEvent): Boolean {
        val conn = connection ?: return true
        val m = mapper ?: return true
        val t = m.convert(ev, SystemClock.elapsedRealtimeNanos() / 1000) ?: return true
        conn.sendTouch(t)
        return true
    }

    /** Back = disconnect. Android 13+ uses the back-invoked dispatcher (predictive back). */
    private fun registerBack() {
        if (Build.VERSION.SDK_INT >= 33) {
            onBackInvokedDispatcher.registerOnBackInvokedCallback(OnBackInvokedDispatcher.PRIORITY_DEFAULT) {
                disconnect("closed on the tablet")
                finish()
            }
        }
    }

    @SuppressLint("GestureBackNavigation") // Android 10-12 path; 13+ handled in registerBack().
    @Deprecated("Deprecated in Java")
    override fun onBackPressed() {
        disconnect("closed on the tablet")
        @Suppress("DEPRECATION")
        super.onBackPressed()
    }

    // --- status / stats overlay

    private fun showStatus(text: String) {
        status.text = text
        status.visibility = View.VISIBLE
    }

    private fun hideStatus() {
        status.visibility = View.GONE
    }

    private val statsTick = object : Runnable {
        override fun run() {
            val d = decoder
            val c = connection
            if (prefs.showStats && d != null && c != null) {
                stats.visibility = View.VISIBLE
                val cfg = stream?.stream
                stats.text = "%s · %.0f fps · latency %.0f ms · decode %.1f ms · rtt %.1f ms · %s".format(
                    cfg?.let { "${it.width}×${it.height}@${it.maxFps}" } ?: "",
                    d.fps, d.lastLatencyMs, d.lastDecodeMs, c.rttUs / 1000f,
                    c.serverFingerprint?.let { PinningTrustManager.short(it) } ?: "",
                )
            } else {
                stats.visibility = View.GONE
            }
            if (!stopped) main.postDelayed(this, 500)
        }
    }

    companion object {
        const val EXTRA_HOST = "host"
        const val EXTRA_PORT = "port"
        const val EXTRA_NAME = "name"
        const val EXTRA_USB = "usb"
    }
}
