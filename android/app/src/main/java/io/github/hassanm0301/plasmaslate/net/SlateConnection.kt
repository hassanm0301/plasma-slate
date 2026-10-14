package io.github.hassanm0301.plasmaslate.net

import android.os.SystemClock
import android.util.Log
import io.github.hassanm0301.plasmaslate.data.Endpoint
import io.github.hassanm0301.plasmaslate.data.Prefs
import io.github.hassanm0301.plasmaslate.proto.ClientHello
import io.github.hassanm0301.plasmaslate.proto.ClientStats
import io.github.hassanm0301.plasmaslate.proto.Envelope
import io.github.hassanm0301.plasmaslate.proto.ErrorCode
import io.github.hassanm0301.plasmaslate.proto.Goodbye
import io.github.hassanm0301.plasmaslate.proto.Link
import io.github.hassanm0301.plasmaslate.proto.Ping
import io.github.hassanm0301.plasmaslate.proto.Pong
import io.github.hassanm0301.plasmaslate.proto.ProtocolVersion
import io.github.hassanm0301.plasmaslate.proto.Ready
import io.github.hassanm0301.plasmaslate.proto.RequestKeyframe
import io.github.hassanm0301.plasmaslate.proto.ServerHello
import io.github.hassanm0301.plasmaslate.proto.TouchEvent
import io.github.hassanm0301.plasmaslate.proto.VideoFrame
import java.io.BufferedInputStream
import java.io.BufferedOutputStream
import java.io.DataInputStream
import java.io.IOException
import java.net.InetSocketAddress
import java.net.Socket
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import javax.net.ssl.SSLContext
import javax.net.ssl.SSLHandshakeException
import javax.net.ssl.SSLSocket

/**
 * One connection to slated. All callbacks run on the connection's network thread; the
 * listener must hand UI work over to the main thread.
 */
class SlateConnection(
    private val endpoint: Endpoint,
    private val prefs: Prefs,
    private val hello: ClientHello.Builder,
    private val listener: Listener,
) {
    interface Listener {
        fun onPairingPending(fingerprintShort: String)
        fun onSessionStarted(hello: ServerHello)
        fun onVideoFrame(frame: VideoFrame, receivedNs: Long)
        /** [fatal] errors should not trigger automatic reconnection. */
        fun onEnded(reason: String, fatal: Boolean, code: ErrorCode?)
    }

    /** Statistics supplied by the decoder for periodic [ClientStats]. */
    interface StatsSource {
        fun snapshot(): ClientStats.Builder
    }

    private val running = AtomicBoolean(true)
    private val outbox = LinkedBlockingQueue<Envelope>(512)
    private var socket: SSLSocket? = null
    private var thread: Thread? = null
    @Volatile var statsSource: StatsSource? = null
    @Volatile var rttUs: Long = 0
        private set
    @Volatile private var bytesReceived: Long = 0
    @Volatile var serverFingerprint: String? = null
        private set
    private var ended = AtomicBoolean(false)

    fun start() {
        thread = Thread({ run() }, "slate-net").apply { start() }
    }

    /** Closes the connection; safe from any thread. */
    fun stop(reason: String = "closed on the tablet") {
        if (!running.getAndSet(false)) return
        offer(Envelope.newBuilder().setGoodbye(Goodbye.newBuilder().setReason(reason)).build())
        Thread({
            Thread.sleep(150)
            try { socket?.close() } catch (_: IOException) {}
        }, "slate-close").start()
    }

    fun sendTouch(ev: TouchEvent) = offer(Envelope.newBuilder().setTouchEvent(ev).build())

    fun sendReady() = offer(Envelope.newBuilder().setReady(Ready.getDefaultInstance()).build())

    fun requestKeyframe(reason: String) =
        offer(Envelope.newBuilder().setRequestKeyframe(RequestKeyframe.newBuilder().setReason(reason)).build())

    private fun offer(env: Envelope) {
        if (!outbox.offer(env)) Log.w(TAG, "outbox full, dropping ${env.msgCase}")
    }

    private fun end(reason: String, fatal: Boolean, code: ErrorCode? = null) {
        if (ended.getAndSet(true)) return
        running.set(false)
        try { socket?.close() } catch (_: IOException) {}
        listener.onEnded(reason, fatal, code)
    }

    private fun run() {
        val trust = PinningTrustManager(prefs.pinnedFingerprint(endpoint))
        try {
            val raw = Socket()
            raw.tcpNoDelay = true
            raw.receiveBufferSize = 1 shl 20
            raw.connect(InetSocketAddress(endpoint.host, endpoint.port), 5000)
            val ctx = SSLContext.getInstance("TLSv1.3")
            ctx.init(null, arrayOf(trust), null)
            val ssl = ctx.socketFactory.createSocket(raw, endpoint.host, endpoint.port, true) as SSLSocket
            ssl.enabledProtocols = arrayOf("TLSv1.3")
            ssl.soTimeout = 0
            socket = ssl
            ssl.startHandshake()
        } catch (e: SSLHandshakeException) {
            val changed = generateSequence<Throwable>(e) { it.cause }.any { it is PinningTrustManager.IdentityChangedException }
            end(if (changed) "identity-changed" else "TLS handshake failed: ${e.message}", fatal = changed)
            return
        } catch (e: IOException) {
            end("cannot reach ${endpoint.host}:${endpoint.port} (${e.message})", fatal = false)
            return
        }
        val fp = trust.seenFingerprint ?: run { end("no server certificate", true); return }
        serverFingerprint = fp
        val sock = socket ?: return

        val writer = Thread({ writeLoop(sock) }, "slate-writer").apply { start() }
        val timer = Thread({ timerLoop() }, "slate-timer").apply { start() }

        offer(
            Envelope.newBuilder().setClientHello(
                hello.setProtocolVersion(ProtocolVersion.PROTOCOL_VERSION_CURRENT_VALUE)
                    .setAuthToken(prefs.token(fp))
                    .setLink(if (endpoint.usb) Link.LINK_USB else Link.LINK_NETWORK),
            ).build(),
        )

        try {
            val input = DataInputStream(BufferedInputStream(sock.inputStream, 1 shl 18))
            while (running.get()) {
                val env = Framing.read(input) ?: break
                bytesReceived += env.serializedSize
                handle(env, fp)
            }
            end("the PC closed the connection", fatal = false)
        } catch (e: IOException) {
            end(if (running.get()) "connection lost (${e.message})" else "disconnected", fatal = !running.get())
        } finally {
            writer.interrupt()
            timer.interrupt()
        }
    }

    private fun handle(env: Envelope, fp: String) {
        when (env.msgCase) {
            Envelope.MsgCase.VIDEO_FRAME -> listener.onVideoFrame(env.videoFrame, SystemClock.elapsedRealtimeNanos())
            Envelope.MsgCase.PING -> offer(
                Envelope.newBuilder().setPong(
                    Pong.newBuilder().setId(env.ping.id).setPingSenderTimeUs(env.ping.senderTimeUs),
                ).build(),
            )
            Envelope.MsgCase.PONG -> {
                val rtt = nowUs() - env.pong.pingSenderTimeUs
                rttUs = if (rttUs == 0L) rtt else (rttUs * 7 + rtt) / 8
            }
            Envelope.MsgCase.PAIRING_PENDING -> listener.onPairingPending(env.pairingPending.serverFingerprint)
            Envelope.MsgCase.SERVER_HELLO -> {
                val sh = env.serverHello
                if (sh.issuedAuthToken.isNotEmpty()) prefs.saveToken(fp, sh.issuedAuthToken)
                prefs.pin(endpoint, fp)
                listener.onSessionStarted(sh)
            }
            Envelope.MsgCase.ERROR -> {
                val code = env.error.code
                val fatal = code == ErrorCode.ERROR_CODE_PAIRING_DENIED ||
                    code == ErrorCode.ERROR_CODE_PROTOCOL_VERSION ||
                    code == ErrorCode.ERROR_CODE_UNAUTHORIZED ||
                    code == ErrorCode.ERROR_CODE_PAIRING_TIMEOUT
                end(env.error.message, fatal, code)
            }
            Envelope.MsgCase.GOODBYE -> end(env.goodbye.reason.ifEmpty { "the PC ended the session" }, fatal = true)
            else -> {}
        }
    }

    private fun writeLoop(sock: SSLSocket) {
        try {
            val out = BufferedOutputStream(sock.outputStream, 1 shl 14)
            while (true) {
                val first = outbox.take()
                out.write(Framing.encode(first))
                // Batch whatever else is already queued into the same flush.
                while (true) {
                    val next = outbox.poll() ?: break
                    out.write(Framing.encode(next))
                }
                out.flush()
                if (first.hasGoodbye()) break
            }
        } catch (_: InterruptedException) {
        } catch (e: IOException) {
            if (running.get()) Log.w(TAG, "write failed: ${e.message}")
        }
    }

    private fun timerLoop() {
        var pingId = 0L
        var lastBytes = 0L
        var lastStats = SystemClock.elapsedRealtime()
        try {
            while (running.get()) {
                TimeUnit.SECONDS.sleep(1)
                pingId++
                if (pingId % 2 == 0L) {
                    offer(Envelope.newBuilder().setPing(Ping.newBuilder().setId(pingId).setSenderTimeUs(nowUs())).build())
                }
                val now = SystemClock.elapsedRealtime()
                val stats = statsSource?.snapshot() ?: ClientStats.newBuilder()
                val b = bytesReceived
                stats.setRttUs(rttUs.toInt()).setBytesReceived(b - lastBytes).setIntervalMs((now - lastStats).toInt())
                lastBytes = b
                lastStats = now
                offer(Envelope.newBuilder().setClientStats(stats).build())
            }
        } catch (_: InterruptedException) {
        }
    }

    companion object {
        private const val TAG = "SlateConnection"
        fun nowUs(): Long = SystemClock.elapsedRealtimeNanos() / 1000
    }
}
