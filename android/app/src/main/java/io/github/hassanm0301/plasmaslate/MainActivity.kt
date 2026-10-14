package io.github.hassanm0301.plasmaslate

import android.app.Activity
import android.app.AlertDialog
import android.content.Intent
import android.os.Bundle
import android.text.InputType
import android.util.TypedValue
import android.view.Gravity
import android.view.View
import android.view.ViewGroup
import android.widget.BaseAdapter
import android.widget.Button
import android.widget.CheckBox
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.ListView
import android.widget.TextView
import io.github.hassanm0301.plasmaslate.data.Endpoint
import io.github.hassanm0301.plasmaslate.data.Prefs
import io.github.hassanm0301.plasmaslate.net.Discovery

/** Lists PCs (USB, discovered, manual) and starts a [StreamActivity]. */
class MainActivity : Activity() {
    private lateinit var prefs: Prefs
    private lateinit var discovery: Discovery
    private lateinit var adapter: ServerAdapter
    private var discovered: List<Endpoint> = emptyList()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        prefs = Prefs(this)
        val pad = dp(24)
        val root = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(pad, pad, pad, pad)
        }
        root.addView(TextView(this).apply {
            text = getString(R.string.servers_title)
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 26f)
        })
        root.addView(TextView(this).apply {
            text = getString(R.string.servers_hint)
            setPadding(0, dp(8), 0, dp(16))
            alpha = 0.8f
        })
        adapter = ServerAdapter()
        val list = ListView(this).apply {
            this.adapter = this@MainActivity.adapter
            setOnItemClickListener { _, _, pos, _ -> connect(this@MainActivity.adapter.getItem(pos)) }
            setOnItemLongClickListener { _, _, pos, _ ->
                val ep = this@MainActivity.adapter.getItem(pos)
                AlertDialog.Builder(this@MainActivity)
                    .setTitle(ep.name)
                    .setMessage("${ep.host}:${ep.port}")
                    .setPositiveButton(R.string.forget) { _, _ -> prefs.forget(ep); refresh() }
                    .setNegativeButton(R.string.cancel, null)
                    .show()
                true
            }
        }
        root.addView(list, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, 0, 1f))
        val bottom = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
        }
        bottom.addView(Button(this).apply {
            text = getString(R.string.add_server)
            setOnClickListener { addServerDialog() }
        })
        bottom.addView(CheckBox(this).apply {
            text = getString(R.string.show_stats)
            isChecked = prefs.showStats
            setOnCheckedChangeListener { _, c -> prefs.showStats = c }
            setPadding(dp(16), 0, 0, 0)
        })
        root.addView(bottom)
        setContentView(root)
        discovery = Discovery(this) { discovered = it; refresh() }
        refresh()
        handleConnectIntent(intent)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handleConnectIntent(intent)
    }

    /**
     * `am start -n io.github.hassanm0301.plasmaslate/.MainActivity --es connect_host 127.0.0.1
     *  [--ei connect_port 47420] [--ez connect_usb true]` connects immediately. slated uses this to
     * open the display when a tablet is plugged in over USB.
     */
    private fun handleConnectIntent(intent: Intent?) {
        val host = intent?.getStringExtra(EXTRA_CONNECT_HOST) ?: return
        val port = intent.getIntExtra(EXTRA_CONNECT_PORT, Prefs.DEFAULT_PORT)
        val usb = intent.getBooleanExtra(EXTRA_CONNECT_USB, host == "127.0.0.1")
        intent.removeExtra(EXTRA_CONNECT_HOST)
        connect(Endpoint(host, port, if (usb) getString(R.string.usb_entry) else host, usb), intent.extras)
    }

    override fun onResume() {
        super.onResume()
        discovery.start()
        refresh()
    }

    override fun onPause() {
        super.onPause()
        discovery.stop()
    }

    private fun refresh() {
        val usb = Endpoint("127.0.0.1", Prefs.DEFAULT_PORT, getString(R.string.usb_entry), usb = true)
        val manual = prefs.manualServers().filter { m -> discovered.none { it.key == m.key } }
        adapter.items = listOf(usb) + discovered + manual
    }

    private fun connect(ep: Endpoint, debugExtras: Bundle? = null) {
        prefs.lastEndpoint = ep.key
        val i = Intent(this, StreamActivity::class.java)
        // Developer options (codec / decoder experiments) are passed through unchanged.
        debugExtras?.keySet()?.filter { it.startsWith("debug_") }?.forEach { k ->
            when (val v = debugExtras.get(k)) {
                is String -> i.putExtra(k, v)
                is Boolean -> i.putExtra(k, v)
                is Int -> i.putExtra(k, v)
            }
        }
        startActivity(
            i.putExtra(StreamActivity.EXTRA_HOST, ep.host)
                .putExtra(StreamActivity.EXTRA_PORT, ep.port)
                .putExtra(StreamActivity.EXTRA_NAME, ep.name)
                .putExtra(StreamActivity.EXTRA_USB, ep.usb),
        )
    }

    private fun addServerDialog() {
        val input = EditText(this).apply {
            hint = getString(R.string.add_server_hint)
            inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_URI
        }
        AlertDialog.Builder(this)
            .setTitle(R.string.add_server)
            .setView(input)
            .setPositiveButton(R.string.connect) { _, _ ->
                Prefs.parse(input.text.toString())?.let {
                    prefs.addManual(it)
                    refresh()
                    connect(it)
                }
            }
            .setNegativeButton(R.string.cancel, null)
            .show()
    }

    private fun dp(v: Int) = (v * resources.displayMetrics.density).toInt()

    companion object {
        const val EXTRA_CONNECT_HOST = "connect_host"
        const val EXTRA_CONNECT_PORT = "connect_port"
        const val EXTRA_CONNECT_USB = "connect_usb"
    }

    private inner class ServerAdapter : BaseAdapter() {
        var items: List<Endpoint> = emptyList()
            set(v) {
                field = v
                notifyDataSetChanged()
            }

        override fun getCount() = items.size
        override fun getItem(position: Int) = items[position]
        override fun getItemId(position: Int) = position.toLong()

        override fun getView(position: Int, convertView: View?, parent: ViewGroup?): View {
            val ep = items[position]
            val v = (convertView as? LinearLayout) ?: LinearLayout(this@MainActivity).apply {
                orientation = LinearLayout.VERTICAL
                setPadding(dp(8), dp(14), dp(8), dp(14))
                addView(TextView(context).apply { setTextSize(TypedValue.COMPLEX_UNIT_SP, 20f) })
                addView(TextView(context).apply { alpha = 0.7f })
            }
            val title = v.getChildAt(0) as TextView
            val detail = v.getChildAt(1) as TextView
            val last = if (prefs.lastEndpoint == ep.key) "  ★" else ""
            title.text = ep.name + last
            detail.text = when {
                ep.usb -> getString(R.string.usb_entry_detail)
                else -> "${ep.host}:${ep.port}" + if (prefs.pinnedFingerprint(ep) != null) "  · paired" else ""
            }
            return v
        }
    }
}
