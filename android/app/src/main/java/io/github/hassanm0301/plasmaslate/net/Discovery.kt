package io.github.hassanm0301.plasmaslate.net

import android.content.Context
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.net.wifi.WifiManager
import android.os.Handler
import android.os.Looper
import android.util.Log
import io.github.hassanm0301.plasmaslate.data.Endpoint

/** Finds slated instances on the LAN (DNS-SD `_plasmaslate._tcp`). Callbacks on the main thread. */
class Discovery(context: Context, private val onChange: (List<Endpoint>) -> Unit) {
    private val nsd = context.getSystemService(Context.NSD_SERVICE) as NsdManager
    private val wifi = context.applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
    private val main = Handler(Looper.getMainLooper())
    private val found = LinkedHashMap<String, Endpoint>()
    private val resolveQueue = ArrayDeque<NsdServiceInfo>()
    private var resolving = false
    private var multicast: WifiManager.MulticastLock? = null
    private var listener: NsdManager.DiscoveryListener? = null

    fun start() {
        if (listener != null) return
        multicast = wifi.createMulticastLock("plasma-slate-mdns").apply { setReferenceCounted(false); acquire() }
        val l = object : NsdManager.DiscoveryListener {
            override fun onStartDiscoveryFailed(serviceType: String?, errorCode: Int) {
                Log.w(TAG, "discovery failed $errorCode")
            }
            override fun onStopDiscoveryFailed(serviceType: String?, errorCode: Int) {}
            override fun onDiscoveryStarted(serviceType: String?) {}
            override fun onDiscoveryStopped(serviceType: String?) {}
            override fun onServiceFound(info: NsdServiceInfo) {
                main.post { enqueue(info) }
            }
            override fun onServiceLost(info: NsdServiceInfo) {
                main.post {
                    if (found.remove(info.serviceName) != null) onChange(found.values.toList())
                }
            }
        }
        listener = l
        nsd.discoverServices(SERVICE_TYPE, NsdManager.PROTOCOL_DNS_SD, l)
    }

    fun stop() {
        listener?.let { runCatching { nsd.stopServiceDiscovery(it) } }
        listener = null
        multicast?.release()
        multicast = null
    }

    private fun enqueue(info: NsdServiceInfo) {
        resolveQueue.addLast(info)
        resolveNext()
    }

    // Android 10 allows only one resolve at a time.
    private fun resolveNext() {
        if (resolving) return
        val info = resolveQueue.removeFirstOrNull() ?: return
        resolving = true
        @Suppress("DEPRECATION")
        nsd.resolveService(info, object : NsdManager.ResolveListener {
            override fun onResolveFailed(serviceInfo: NsdServiceInfo?, errorCode: Int) {
                main.post { resolving = false; resolveNext() }
            }

            override fun onServiceResolved(r: NsdServiceInfo) {
                main.post {
                    resolving = false
                    @Suppress("DEPRECATION")
                    val host = r.host?.hostAddress
                    if (host != null) {
                        val name = r.attributes["name"]?.let { String(it) } ?: r.serviceName
                        found[r.serviceName] = Endpoint(host, r.port, name)
                        onChange(found.values.toList())
                    }
                    resolveNext()
                }
            }
        })
    }

    companion object {
        private const val TAG = "Discovery"
        const val SERVICE_TYPE = "_plasmaslate._tcp."
    }
}
