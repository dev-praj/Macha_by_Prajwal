package com.macha.android.link

import android.annotation.SuppressLint
import java.net.URI
import java.net.URLDecoder
import java.security.MessageDigest
import java.security.cert.CertificateException
import java.security.cert.X509Certificate
import javax.net.ssl.X509TrustManager

/**
 * What the desktop shows as a QR code: macha://pair?v=1&host=...&port=...&fp=...&token=...&name=...
 * (and, when its relay is on, &relay=...&room=...&key=...&access=..., see docs/RELAY_LINK.md).
 * `fp` is the SHA-256 (hex) of the desktop's TLS certificate; `token` is the shared pairing secret.
 */
data class PairingPayload(
    val host: String,
    val port: Int,
    val certSha256: String,
    val token: String,
    val desktopName: String,
    /** The relay fields of the link (all four or none); null for a link that only works on the local network. */
    val relay: RelayPairing? = null,
    /** Optional Supabase pairing fields, all present together; local-only QR links remain unchanged. */
    val cloud: SupabasePairing? = null,
    /** Account-authenticated Remote route; never parsed from a QR code. */
    val accountRemote: Boolean = false,
) {
    companion object {
        /** Returns null for anything that is not a well-formed v1 pairing link. */
        fun parse(text: String): PairingPayload? {
            val uri = try { URI(text.trim()) } catch (_: Exception) { return null }
            if (uri.scheme != "macha" || uri.host != "pair") return null
            val q = (uri.rawQuery ?: return null).split("&").mapNotNull {
                val i = it.indexOf('=')
                if (i <= 0) null else it.substring(0, i) to URLDecoder.decode(it.substring(i + 1), "UTF-8")
            }.toMap()
            if (q["v"] != Protocol.VERSION.toString()) return null
            val host = q["host"]?.takeIf { it.isNotBlank() && it.length <= 253 } ?: return null
            val port = q["port"]?.toIntOrNull()?.takeIf { it in 1..65535 } ?: return null
            val fp = q["fp"]?.lowercase()?.takeIf(Wire::isFingerprint) ?: return null
            val token = q["token"]?.takeIf { it.length in 16..128 && it.all { c -> c.isLetterOrDigit() || c == '-' || c == '_' } }
                ?: return null
            // The relay fields are all there and well formed, or all absent; anything in between is a damaged link.
            val relay = when (val r = RelayPairing.fromQuery(q)) {
                RelayPairing.Companion.Parsed.Absent -> null
                RelayPairing.Companion.Parsed.Invalid -> return null
                is RelayPairing.Companion.Parsed.Ok -> r.relay
            }
            val cloudFields = listOf("cloudUrl", "cloudKey", "cloudPairingId", "cloudToken", "cloudLaptopPubKey").map { q[it] }
            val cloud = if (cloudFields.all { it == null }) null else {
                if (cloudFields.any { it == null }) return null
                val (url, key, id, cloudToken, pubkey) = cloudFields
                SupabasePairing(url!!, key!!, id!!, cloudToken!!, pubkey!!).takeIf { it.valid() } ?: return null
            }
            return PairingPayload(host, port, fp, token, q["name"].orEmpty().take(64), relay, cloud)
        }
    }
}

/** Accepts exactly one certificate, the one whose SHA-256 was in the QR code. No CA, no hostname check. */
@SuppressLint("CustomX509TrustManager") // intentional: certificate pinning to the paired desktop
class PinnedTrustManager(private val pinnedSha256Hex: String) : X509TrustManager {
    override fun checkServerTrusted(chain: Array<out X509Certificate>?, authType: String?) {
        val leaf = chain?.firstOrNull() ?: throw CertificateException("empty certificate chain")
        val actual = MessageDigest.getInstance("SHA-256").digest(leaf.encoded).joinToString("") { "%02x".format(it) }
        if (!MessageDigest.isEqual(actual.toByteArray(), pinnedSha256Hex.lowercase().toByteArray())) {
            throw CertificateException("certificate does not match the paired desktop")
        }
    }

    override fun checkClientTrusted(chain: Array<out X509Certificate>?, authType: String?) {
        throw CertificateException("client certificates are not used")
    }

    override fun getAcceptedIssuers(): Array<X509Certificate> = emptyArray()
}

/**
 * How a pairing is kept (encrypted by SecureStore): one field per line, the relay's four after the five LAN ones. An older
 * file with five lines reads as a LAN-only pairing, so an app update never loses a pairing.
 */
data class SupabasePairing(
    val url: String,
    val anonKey: String,
    val pairingId: String,
    val token: String,
    val laptopPubKey: String,
    val linkId: String? = null,
    val deviceId: String? = null,
    val laptopDeviceId: String? = null,
    val phonePrivateKey: String? = null,
    val refreshToken: String? = null,
    val sendSeq: Long = 0,
    val receiveSeq: Long = 0,
    val accountUserId: String? = null,
) {
    override fun toString() = "SupabasePairing(..)"
    fun valid(): Boolean = url.matches(Regex("https://[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\\.supabase\\.co"))
        && anonKey.isNotBlank() && anonKey.length <= 4096
        && pairingId.matches(Regex("[0-9a-fA-F-]{36}"))
        && token.matches(Regex("[A-Za-z0-9_-]{43}"))
        && RelayCrypto.base64UrlDecode(laptopPubKey)?.size == 32
}

object PairingCodec {
    fun encode(p: PairingPayload): String {
        val base = listOf(p.host, p.port.toString(), p.certSha256, p.token, p.desktopName)
        val r = p.relay
        val fields = if (r == null) base else base + listOf(r.url, r.room, r.key, r.access)
        val c = p.cloud ?: return fields.joinToString("\n")
        return (fields + listOf("CLOUD1", c.url, c.anonKey, c.pairingId, c.token, c.laptopPubKey,
            c.linkId.orEmpty(), c.deviceId.orEmpty(), c.laptopDeviceId.orEmpty(), c.phonePrivateKey.orEmpty(), c.refreshToken.orEmpty(), c.sendSeq.toString(), c.receiveSeq.toString())).joinToString("\n")
    }

    /** Null when the text is not a pairing this build can use. */
    fun decode(text: String): PairingPayload? {
        val parts = text.split("\n")
        if (parts.size < 4) return null
        val port = parts[1].toIntOrNull() ?: return null
        val relay = if (parts.size >= 9 && parts[5] != "CLOUD1") RelayPairing(parts[5], parts[6], parts[7], parts[8]) else null
        val cloudAt = parts.indexOf("CLOUD1")
        val cloud = if (cloudAt >= 0) {
            if (parts.size < cloudAt + 11) return null
            SupabasePairing(parts[cloudAt + 1], parts[cloudAt + 2], parts[cloudAt + 3], parts[cloudAt + 4], parts[cloudAt + 5],
                parts[cloudAt + 6].ifEmpty { null }, parts[cloudAt + 7].ifEmpty { null }, parts[cloudAt + 8].ifEmpty { null },
                parts[cloudAt + 9].ifEmpty { null }, parts[cloudAt + 10].ifEmpty { null },
                parts.getOrNull(cloudAt + 11)?.toLongOrNull() ?: 0, parts.getOrNull(cloudAt + 12)?.toLongOrNull() ?: 0)
        } else null
        return PairingPayload(parts[0], port, parts[2], parts[3], parts.getOrElse(4) { "" }, relay, cloud)
    }
}
