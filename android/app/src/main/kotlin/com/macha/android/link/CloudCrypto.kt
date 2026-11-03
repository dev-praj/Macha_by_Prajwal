package com.macha.android.link

import org.bouncycastle.math.ec.rfc7748.X25519
import org.json.JSONObject
import java.security.SecureRandom
import javax.crypto.Cipher
import javax.crypto.Mac
import javax.crypto.spec.GCMParameterSpec
import javax.crypto.spec.SecretKeySpec

/** X25519 + HKDF-SHA256 + AES-256-GCM for the Supabase relay envelope. */
object CloudCrypto {
    private val random = SecureRandom()
    private val salt = "macha-supabase-relay-v1".toByteArray(Charsets.UTF_8)
    private const val MAX_AGE_MS = 600_000L

    data class Identity(val privateKey: ByteArray, val publicKey: ByteArray) {
        fun privateEncoded(): String = encode(privateKey)
        fun publicEncoded(): String = encode(publicKey)
    }

    data class Envelope(val id: String, val kind: String, val seq: Long, val ts: Long, val nonce: String, val ciphertext: String) {
        fun json(): JSONObject = JSONObject().put("id", id).put("type", kind).put("seq", seq).put("ts", ts).put("nonce", nonce).put("ciphertext", ciphertext)
        companion object {
            fun parse(value: JSONObject) = Envelope(value.getString("id"), value.getString("type"), value.getLong("seq"), value.getLong("ts"), value.getString("nonce"), value.getString("ciphertext"))
        }
    }

    class Codec(key: ByteArray, private var sent: Long = 0L, private var received: Long = 0L) {
        private val key = key.copyOf()
        @Synchronized fun counters(): Pair<Long, Long> = sent to received

        @Synchronized fun seal(text: String, kind: String = "data"): Envelope {
            check(sent < Long.MAX_VALUE) { "cloud message counter exhausted" }
            val seq = ++sent
            val ts = System.currentTimeMillis()
            val id = java.util.UUID.randomUUID().toString()
            val nonce = ByteArray(12).also(random::nextBytes)
            val cipher = Cipher.getInstance("AES/GCM/NoPadding")
            cipher.init(Cipher.ENCRYPT_MODE, SecretKeySpec(key, "AES"), GCMParameterSpec(128, nonce))
            cipher.updateAAD(aad(id, kind, seq, ts))
            return Envelope(id, kind, seq, ts, encode(nonce), encode(cipher.doFinal(text.toByteArray(Charsets.UTF_8))))
        }

        @Synchronized fun open(envelope: Envelope): String {
            require(envelope.seq > received) { "duplicate or out-of-order cloud message" }
            require(kotlin.math.abs(System.currentTimeMillis() - envelope.ts) <= MAX_AGE_MS) { "cloud message expired" }
            val nonce = decode(envelope.nonce).also { require(it.size == 12) { "invalid nonce" } }
            val cipher = Cipher.getInstance("AES/GCM/NoPadding")
            cipher.init(Cipher.DECRYPT_MODE, SecretKeySpec(key, "AES"), GCMParameterSpec(128, nonce))
            cipher.updateAAD(aad(envelope.id, envelope.kind, envelope.seq, envelope.ts))
            val text = cipher.doFinal(decode(envelope.ciphertext)).toString(Charsets.UTF_8)
            received = envelope.seq
            return text
        }

        fun wipe() { key.fill(0) }
    }

    fun generateIdentity(): Identity {
        val privateKey = ByteArray(32).also(random::nextBytes)
        val publicKey = ByteArray(32)
        X25519.generatePublicKey(privateKey, 0, publicKey, 0)
        return Identity(privateKey, publicKey)
    }

    fun identityFromPrivate(encoded: String): Identity {
        val privateKey = decode(encoded).also { require(it.size == 32) { "invalid X25519 private key" } }
        val publicKey = ByteArray(32)
        X25519.generatePublicKey(privateKey, 0, publicKey, 0)
        return Identity(privateKey, publicKey)
    }

    fun derive(identity: Identity, peerPublic: String, linkId: String): ByteArray {
        val peer = decode(peerPublic).also { require(it.size == 32) { "invalid X25519 peer key" } }
        val secret = ByteArray(32)
        require(X25519.calculateAgreement(identity.privateKey, 0, peer, 0, secret, 0)) { "invalid X25519 peer key" }
        require(secret.any { it.toInt() != 0 }) { "invalid X25519 shared key" }
        val prk = hmac(salt, secret)
        val info = "link:$linkId".toByteArray(Charsets.UTF_8)
        val expanded = hmac(prk, info + byteArrayOf(1)).copyOf(32)
        secret.fill(0)
        prk.fill(0)
        return expanded
    }

    fun encode(bytes: ByteArray): String = java.util.Base64.getUrlEncoder().withoutPadding().encodeToString(bytes)
    fun decode(value: String): ByteArray = java.util.Base64.getUrlDecoder().decode(value)

    private fun hmac(key: ByteArray, data: ByteArray): ByteArray = Mac.getInstance("HmacSHA256").run {
        init(SecretKeySpec(key, "HmacSHA256"))
        doFinal(data)
    }

    private fun aad(id: String, kind: String, seq: Long, ts: Long) = "$id\n$kind\n$seq\n$ts".toByteArray(Charsets.UTF_8)
}
