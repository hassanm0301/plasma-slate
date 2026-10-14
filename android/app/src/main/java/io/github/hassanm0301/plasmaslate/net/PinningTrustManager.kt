package io.github.hassanm0301.plasmaslate.net

import android.annotation.SuppressLint
import java.security.MessageDigest
import java.security.cert.CertificateException
import java.security.cert.X509Certificate
import javax.net.ssl.X509TrustManager

/**
 * Trust on first use: accepts any server certificate when [pinned] is null (the user then
 * confirms the pairing on the PC), otherwise only the certificate with that SHA-256 fingerprint.
 *
 * Intentionally custom: the server is self-signed and identified by its pinned fingerprint,
 * never by a CA chain (see docs/adr and slate-net/src/tls.rs for the matching server side).
 */
@SuppressLint("CustomX509TrustManager")
class PinningTrustManager(private val pinned: String?) : X509TrustManager {
    @Volatile
    var seenFingerprint: String? = null
        private set

    override fun checkServerTrusted(chain: Array<out X509Certificate>?, authType: String?) {
        val leaf = chain?.firstOrNull() ?: throw CertificateException("no server certificate")
        val fp = fingerprint(leaf.encoded)
        seenFingerprint = fp
        if (pinned != null && !pinned.equals(fp, ignoreCase = true)) {
            throw IdentityChangedException(fp)
        }
    }

    override fun checkClientTrusted(chain: Array<out X509Certificate>?, authType: String?) {
        throw CertificateException("client certificates are not used")
    }

    override fun getAcceptedIssuers(): Array<X509Certificate> = emptyArray()

    class IdentityChangedException(val fingerprint: String) : CertificateException("server certificate changed: $fingerprint")

    companion object {
        fun fingerprint(der: ByteArray): String =
            MessageDigest.getInstance("SHA-256").digest(der).joinToString("") { "%02x".format(it) }

        /** `abcd1234…` → `ABCD-1234`, matching the PC's pairing prompt. */
        fun short(fp: String): String {
            val s = fp.take(8).uppercase()
            return if (s.length == 8) s.substring(0, 4) + "-" + s.substring(4) else s
        }
    }
}
