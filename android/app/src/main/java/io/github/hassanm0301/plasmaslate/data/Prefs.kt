package io.github.hassanm0301.plasmaslate.data

import android.content.Context
import java.util.UUID

/** A PC the user can connect to. */
data class Endpoint(val host: String, val port: Int, val name: String, val usb: Boolean = false) {
    val key: String get() = "$host:$port"
}

/**
 * Small persistent store:
 *  - device id (random, per install)
 *  - pinned certificate fingerprint per endpoint (host:port)
 *  - auth token per server fingerprint (shared by USB and network routes to the same PC)
 *  - manually added servers, last used endpoint, UI options
 */
class Prefs(context: Context) {
    private val sp = context.getSharedPreferences("plasma-slate", Context.MODE_PRIVATE)

    val deviceId: String
        get() = sp.getString(KEY_DEVICE_ID, null) ?: UUID.randomUUID().toString().also {
            sp.edit().putString(KEY_DEVICE_ID, it).apply()
        }

    fun pinnedFingerprint(endpoint: Endpoint): String? = sp.getString("pin:${endpoint.key}", null)

    fun pin(endpoint: Endpoint, fingerprint: String) {
        sp.edit().putString("pin:${endpoint.key}", fingerprint).apply()
    }

    fun token(fingerprint: String): String = sp.getString("token:$fingerprint", "") ?: ""

    fun saveToken(fingerprint: String, token: String) {
        sp.edit().putString("token:$fingerprint", token).apply()
    }

    /** Forgets pinning (and the token of the pinned identity) for an endpoint. */
    fun forget(endpoint: Endpoint) {
        val fp = pinnedFingerprint(endpoint)
        val e = sp.edit().remove("pin:${endpoint.key}")
        if (fp != null) e.remove("token:$fp")
        e.apply()
        removeManual(endpoint)
    }

    fun manualServers(): List<Endpoint> =
        (sp.getStringSet(KEY_MANUAL, emptySet()) ?: emptySet()).mapNotNull { parse(it) }.sortedBy { it.key }

    fun addManual(endpoint: Endpoint) {
        val set = (sp.getStringSet(KEY_MANUAL, emptySet()) ?: emptySet()).toMutableSet()
        set.add(endpoint.key)
        sp.edit().putStringSet(KEY_MANUAL, set).apply()
    }

    private fun removeManual(endpoint: Endpoint) {
        val set = (sp.getStringSet(KEY_MANUAL, emptySet()) ?: emptySet()).toMutableSet()
        if (set.remove(endpoint.key)) sp.edit().putStringSet(KEY_MANUAL, set).apply()
    }

    var lastEndpoint: String?
        get() = sp.getString(KEY_LAST, null)
        set(v) = sp.edit().putString(KEY_LAST, v).apply()

    var showStats: Boolean
        get() = sp.getBoolean(KEY_STATS, false)
        set(v) = sp.edit().putBoolean(KEY_STATS, v).apply()

    // --- latency options (see README "Latency tuning")

    /** Which decoder to use: hardware holds frames on some SoCs, software never does but costs CPU. */
    var decoderMode: DecoderMode
        get() = DecoderMode.entries.firstOrNull { it.key == sp.getString(KEY_DECODER, null) } ?: DecoderMode.AUTO
        set(v) = sp.edit().putString(KEY_DECODER, v.key).apply()

    /** Vendor-specific low-latency decoder keys (AUTO = on except on HiSilicon, where they hurt). */
    var vendorLowLatency: Tristate
        get() = Tristate.entries.firstOrNull { it.key == sp.getString(KEY_VENDOR_LL, null) } ?: Tristate.AUTO
        set(v) = sp.edit().putString(KEY_VENDOR_LL, v.key).apply()

    /** Append an access-unit delimiter to every frame so the decoder knows it is complete. */
    var trailingAud: Boolean
        get() = sp.getBoolean(KEY_TRAILING_AUD, false)
        set(v) = sp.edit().putBoolean(KEY_TRAILING_AUD, v).apply()

    /** Run the panel at its highest refresh rate while streaming. */
    var maxRefreshRate: Boolean
        get() = sp.getBoolean(KEY_MAX_REFRESH, true)
        set(v) = sp.edit().putBoolean(KEY_MAX_REFRESH, v).apply()

    enum class DecoderMode(val key: String) { AUTO("auto"), HARDWARE("hardware"), SOFTWARE("software") }
    enum class Tristate(val key: String) { AUTO("auto"), ON("on"), OFF("off") }

    companion object {
        const val DEFAULT_PORT = 47420
        private const val KEY_DEVICE_ID = "device_id"
        private const val KEY_MANUAL = "manual_servers"
        private const val KEY_LAST = "last_endpoint"
        private const val KEY_STATS = "show_stats"
        private const val KEY_DECODER = "decoder_mode"
        private const val KEY_VENDOR_LL = "vendor_low_latency"
        private const val KEY_TRAILING_AUD = "trailing_aud"
        private const val KEY_MAX_REFRESH = "max_refresh_rate"

        /** Parses `host` or `host:port` (IPv6 in brackets). */
        fun parse(text: String, name: String? = null): Endpoint? {
            val t = text.trim()
            if (t.isEmpty()) return null
            val (host, port) = when {
                t.startsWith("[") -> {
                    val end = t.indexOf(']')
                    if (end < 0) return null
                    val h = t.substring(1, end)
                    val rest = t.substring(end + 1)
                    h to (if (rest.startsWith(":")) rest.drop(1).toIntOrNull() ?: return null else DEFAULT_PORT)
                }
                t.count { it == ':' } == 1 -> {
                    val (h, p) = t.split(':')
                    h to (p.toIntOrNull() ?: return null)
                }
                else -> t to DEFAULT_PORT
            }
            if (host.isBlank() || port !in 1..65535) return null
            return Endpoint(host, port, name ?: host)
        }
    }
}
