package com.macha.android.link

import org.json.JSONArray
import org.json.JSONObject
import java.io.ByteArrayOutputStream
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.net.HttpURLConnection
import java.net.InetSocketAddress
import java.net.Socket
import java.net.URL
import java.net.URLEncoder
import java.net.SocketTimeoutException
import java.security.MessageDigest
import java.security.SecureRandom
import android.util.Base64
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit

data class CloudSession(val access: String, val refresh: String, val userId: String, val email: String)

/** Supabase Google Auth and the small PostgREST surface used by phone pairing and the cloud pipe. */
object SupabaseCloud {
    private const val MAX_REST_BYTES = 1_048_576

    fun newPkceVerifier(): String {
        val bytes = ByteArray(32).also(SecureRandom()::nextBytes)
        return Base64.encodeToString(bytes, Base64.URL_SAFE or Base64.NO_WRAP or Base64.NO_PADDING)
    }

    fun googleAuthorizeUrl(pairing: SupabasePairing, verifier: String): String {
        val challenge = Base64.encodeToString(MessageDigest.getInstance("SHA-256").digest(verifier.toByteArray(Charsets.UTF_8)), Base64.URL_SAFE or Base64.NO_WRAP or Base64.NO_PADDING)
        val base = URL(pairing.url)
        val query = listOf(
            "provider=google", "apikey=${enc(pairing.anonKey)}", "redirect_to=${enc("macha://auth/callback")}",
            "code_challenge=${enc(challenge)}", "code_challenge_method=s256", "scopes=${enc("openid email profile")}",
            "query_params=${enc("{\"prompt\":\"select_account\"}")}",
        ).joinToString("&")
        return "${base.protocol}://${base.host}/auth/v1/authorize?$query"
    }

    fun exchangeGoogleCode(pairing: SupabasePairing, code: String, verifier: String): CloudSession {
        val session = auth(pairing.url, pairing.anonKey, "token?grant_type=pkce", JSONObject().put("auth_code", code).put("code_verifier", verifier))
        return session
    }

    fun refreshGoogleSession(pairing: SupabasePairing, refresh: String): CloudSession = refresh(pairing.url, pairing.anonKey, refresh)

    fun claim(pairing: SupabasePairing, session: CloudSession): SupabasePairing {
        val identity = CloudCrypto.generateIdentity()
        val result = rpc(pairing.url, pairing.anonKey, session.access, "pair_claim", JSONObject()
            .put("pairing_id", pairing.pairingId)
            .put("token", pairing.token)
            .put("phone_pubkey", identity.publicEncoded()))
        result.optString("error").takeIf { it.isNotBlank() }?.let {
            val detail = if (it == "same_google_account_required") "The phone and laptop must use the same Google account." else it
            throw IOException("Pairing failed: $detail")
        }
        val linkId = result.optString("link_id").takeIf { it.isNotBlank() } ?: throw IOException("Pairing response did not include a link id")
        val deviceId = result.optString("device_id").takeIf { it.isNotBlank() } ?: throw IOException("Pairing response did not include the phone device id")
        val laptopDeviceId = result.optString("laptop_device_id").takeIf { it.isNotBlank() } ?: throw IOException("Pairing response did not include the laptop device id")
        return pairing.copy(linkId = linkId, deviceId = deviceId, laptopDeviceId = laptopDeviceId,
            phonePrivateKey = identity.privateEncoded(), refreshToken = session.refresh)
    }

    internal fun open(pairing: SupabasePairing, timeoutMs: Int, onRefresh: (String) -> Unit, onCounters: (Long, Long) -> Unit,
                      sessionProvider: (() -> CloudSession)? = null): Socket {
        val linkId = pairing.linkId ?: throw IOException("Cloud pairing is not complete")
        val deviceId = pairing.deviceId ?: throw IOException("Cloud device id is missing")
        val peerId = pairing.laptopDeviceId ?: throw IOException("Laptop device id is missing")
        val privateKey = pairing.phonePrivateKey ?: throw IOException("Phone cloud key is missing")
        val refresh = pairing.refreshToken ?: throw IOException("Phone cloud session is missing")
        val identity = CloudCrypto.identityFromPrivate(privateKey)
        val key = CloudCrypto.derive(identity, pairing.laptopPubKey, linkId)
        val session = sessionProvider?.invoke() ?: refresh(pairing.url, pairing.anonKey, refresh)
        onRefresh(session.refresh)
        val ref = URL(pairing.url)
        val host = ref.host
        val apiKey = URLEncoder.encode(pairing.anonKey, Charsets.UTF_8.name()).replace("+", "%20")
        val url = RelayUrl(true, host, 443)
        val ws = WsClient.open(url,
            "/realtime/v1/websocket?apikey=$apiKey&vsn=1.0.0",
            listOf("Authorization" to "Bearer ${session.access}"), emptyList(), timeoutMs, 1_048_576)
        try {
            join(ws, linkId, session.access, pairing.accountUserId)
            return CloudStreamSocket(pairing, linkId, deviceId, peerId, session, key, ws, onCounters)
        } catch (e: Throwable) {
            ws.close()
            key.fill(0)
            throw e
        }
    }

    private fun refresh(url: String, key: String, refresh: String): CloudSession = auth(url, key, "token?grant_type=refresh_token", JSONObject().put("refresh_token", refresh))

    private fun auth(url: String, key: String, path: String, body: JSONObject): CloudSession {
        val result = request("POST", "$url/auth/v1/$path", key, null, body)
        val user = result.optJSONObject("user") ?: throw IOException("Supabase did not return the signed-in user")
        if (user.optJSONObject("app_metadata")?.optString("provider") != "google" || user.optString("email_confirmed_at").isBlank()) {
            throw IOException("Sign in with a verified Google account. Anonymous or other accounts cannot use cloud pairing.")
        }
        return CloudSession(result.optString("access_token").takeIf { it.isNotBlank() } ?: throw IOException("Supabase did not return an access token"),
            result.optString("refresh_token").takeIf { it.isNotBlank() } ?: throw IOException("Supabase did not return a refresh token"),
            user.optString("id").takeIf { it.isNotBlank() } ?: throw IOException("Google sign-in did not return a user ID"),
            user.optString("email").takeIf { it.isNotBlank() } ?: throw IOException("Google sign-in did not return an email"))
    }

    private fun rpc(url: String, key: String, jwt: String, name: String, body: JSONObject): JSONObject = request("POST", "$url/rest/v1/rpc/$name", key, jwt, body)

    private fun request(method: String, url: String, key: String, jwt: String?, body: JSONObject? = null): JSONObject {
        val connection = URL(url).openConnection() as HttpURLConnection
        try {
            connection.requestMethod = method
            connection.connectTimeout = 15_000
            connection.readTimeout = 15_000
            connection.setRequestProperty("apikey", key)
            if (jwt != null) connection.setRequestProperty("Authorization", "Bearer $jwt")
            if (body != null) {
                connection.doOutput = true
                connection.setRequestProperty("Content-Type", "application/json")
                connection.outputStream.use { it.write(body.toString().toByteArray(Charsets.UTF_8)) }
            }
            val code = connection.responseCode
            val stream = if (code in 200..299) connection.inputStream else connection.errorStream
            val bytes = stream?.use { input ->
                val out = ByteArrayOutputStream()
                val buf = ByteArray(8192)
                while (true) {
                    val n = input.read(buf)
                    if (n < 0) break
                    if (out.size() + n > MAX_REST_BYTES) throw IOException("Supabase response is too large")
                    out.write(buf, 0, n)
                }
                out.toByteArray()
            } ?: ByteArray(0)
            if (code !in 200..299) {
                val message = runCatching { JSONObject(String(bytes, Charsets.UTF_8)).optString("msg").ifBlank { JSONObject(String(bytes, Charsets.UTF_8)).optString("message") } }.getOrDefault("")
                throw IOException("Supabase returned HTTP $code${if (message.isBlank()) "" else ": ${message.take(180)}"}")
            }
            return if (bytes.isEmpty()) JSONObject() else JSONObject(String(bytes, Charsets.UTF_8))
        } finally { connection.disconnect() }
    }

    private fun join(ws: WsConnection, linkId: String, jwt: String, accountUserId: String?) {
        val topic = "realtime:${accountUserId?.let { "user:$it" } ?: "link:$linkId"}"
        ws.sendText(JSONObject().put("topic", topic).put("event", "phx_join").put("ref", "join-1").put("join_ref", "join-1")
            .put("payload", JSONObject().put("access_token", jwt).put("config", JSONObject()
                .put("private", true).put("broadcast", JSONObject().put("ack", true).put("self", false))))
            .toString())
        val deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(12)
        while (System.nanoTime() < deadline) {
            ws.setReadTimeout(((deadline - System.nanoTime()) / 1_000_000L).toInt().coerceAtLeast(1))
            when (val message = ws.receive()) {
                is WsMessage.Text -> {
                    val value = runCatching { JSONObject(message.text) }.getOrNull() ?: continue
                    if (value.optString("event") != "phx_reply" || value.optString("ref") != "join-1") continue
                    if (value.optJSONObject("payload")?.optString("status") != "ok") {
                        val reason = value.optJSONObject("payload")?.optJSONObject("response")?.optString("reason").orEmpty()
                        throw IOException("Private Realtime channel rejected this device: ${reason.ifBlank { "check the Remote channel policies" }}")
                    }
                    return
                }
                is WsMessage.Closed -> throw IOException("Realtime closed during channel join")
                is WsMessage.Binary -> Unit
            }
        }
        throw SocketTimeoutException("Timed out joining the private Realtime channel")
    }

    private fun insert(pairing: SupabasePairing, session: CloudSession, linkId: String, from: String, to: String, envelope: CloudCrypto.Envelope) {
        pairing.accountUserId?.let { userId ->
            val row = JSONObject().put("id", envelope.id).put("user_id", userId).put("from_device", from).put("to_device", to)
                .put("ciphertext", envelope.json().toString()).put("nonce", envelope.nonce)
                .put("expires_at", java.time.Instant.now().plusSeconds(600).toString())
            request("POST", "${pairing.url}/rest/v1/remote_queued_messages", pairing.anonKey, session.access, row)
            return
        }
        val row = JSONObject().put("id", envelope.id).put("link_id", linkId).put("from_device", from).put("to_device", to)
            .put("message_type", envelope.kind).put("seq", envelope.seq).put("sent_at", envelope.ts)
            .put("ciphertext", envelope.ciphertext).put("nonce", envelope.nonce)
            .put("expires_at", java.time.Instant.now().plusSeconds(600).toString())
        request("POST", "${pairing.url}/rest/v1/queued_messages", pairing.anonKey, session.access, row)
    }

    private fun queued(pairing: SupabasePairing, session: CloudSession, device: String): JSONArray {
        if (pairing.accountUserId != null) {
            val query = "?select=id,ciphertext,nonce&to_device=eq.${enc(device)}&order=created_at.asc"
            val conn = URL("${pairing.url}/rest/v1/remote_queued_messages$query").openConnection() as HttpURLConnection
            try {
                conn.connectTimeout = 10_000; conn.readTimeout = 10_000
                conn.setRequestProperty("apikey", pairing.anonKey); conn.setRequestProperty("Authorization", "Bearer ${session.access}")
                val code = conn.responseCode
                if (code !in 200..299) throw IOException("Remote queue poll returned HTTP $code")
                val body = conn.inputStream.use { it.readBytes() }
                if (body.size > MAX_REST_BYTES) throw IOException("Remote queue response is too large")
                return JSONArray(String(body, Charsets.UTF_8))
            } finally { conn.disconnect() }
        }
        val query = "?select=id,message_type,seq,sent_at,ciphertext,nonce&to_device=eq.${enc(device)}&order=created_at.asc"
        val conn = URL("${pairing.url}/rest/v1/queued_messages$query").openConnection() as HttpURLConnection
        try {
            conn.connectTimeout = 10_000; conn.readTimeout = 10_000
            conn.setRequestProperty("apikey", pairing.anonKey); conn.setRequestProperty("Authorization", "Bearer ${session.access}")
            val code = conn.responseCode
            if (code !in 200..299) throw IOException("Queue poll returned HTTP $code")
            val body = conn.inputStream.use { it.readBytes() }
            if (body.size > MAX_REST_BYTES) throw IOException("Queue response is too large")
            return JSONArray(String(body, Charsets.UTF_8))
        } finally { conn.disconnect() }
    }

    private fun delete(pairing: SupabasePairing, session: CloudSession, id: String) {
        val table = if (pairing.accountUserId != null) "remote_queued_messages" else "queued_messages"
        val conn = URL("${pairing.url}/rest/v1/$table?id=eq.${enc(id)}").openConnection() as HttpURLConnection
        try {
            conn.requestMethod = "DELETE"; conn.connectTimeout = 10_000; conn.readTimeout = 10_000
            conn.setRequestProperty("apikey", pairing.anonKey); conn.setRequestProperty("Authorization", "Bearer ${session.access}")
            val code = conn.responseCode
            if (code !in 200..299) throw IOException("Queue acknowledgement returned HTTP $code")
            conn.inputStream?.close()
        } finally { conn.disconnect() }
    }

    private fun enc(s: String) = URLEncoder.encode(s, Charsets.UTF_8.name())

    private class CloudStreamSocket(
        private val pairing: SupabasePairing,
        private val linkId: String,
        private val deviceId: String,
        private val peerId: String,
        private val session: CloudSession,
        key: ByteArray,
        private val ws: WsConnection,
        private val onCounters: (Long, Long) -> Unit,
    ) : Socket() {
        private val codec = CloudCrypto.Codec(key, pairing.sendSeq, pairing.receiveSeq)
        private val incoming = LinkedBlockingQueue<ByteArray>()
        @Volatile private var closed = false
        @Volatile private var readTimeoutMs = 0
        private var pending = ByteArray(0)
        private var pendingAt = 0
        private var lastHeartbeat = System.currentTimeMillis()
        private var ref = 1
        private val topic = "realtime:${pairing.accountUserId?.let { "user:$it" } ?: "link:$linkId"}"
        private val reader = Thread({ pump() }, "macha-supabase-reader").apply { isDaemon = true; start() }

        private fun pump() {
            try {
                ws.setReadTimeout(1_000)
                while (!closed) {
                    try {
                        when (val msg = ws.receive()) {
                            is WsMessage.Text -> acceptRealtime(msg.text)
                            is WsMessage.Closed -> break
                            is WsMessage.Binary -> Unit
                        }
                    } catch (_: SocketTimeoutException) {
                        pollQueue()
                    }
                    if (System.currentTimeMillis() - lastHeartbeat >= 20_000) {
                        ws.sendText(JSONObject().put("topic", "phoenix").put("event", "heartbeat").put("payload", JSONObject()).put("ref", "hb-${ref++}").toString())
                        lastHeartbeat = System.currentTimeMillis()
                    }
                }
            } catch (_: Exception) {
            } finally { closed = true; incoming.offer(END) }
        }

        private fun acceptRealtime(text: String) {
            val root = runCatching { JSONObject(text) }.getOrNull() ?: return
            val payload = root.optJSONObject("payload") ?: return
            if (root.optString("event") != "broadcast" || payload.optString("event") != "macha_envelope") return
            val envelopeJson = payload.optJSONObject("payload")?.optJSONObject("envelope") ?: return
            acceptEnvelope(CloudCrypto.Envelope.parse(envelopeJson), null)
        }

        private fun pollQueue() {
            val rows = runCatching { queued(pairing, session, deviceId) }.getOrNull() ?: return
            for (i in 0 until rows.length()) {
                val row = rows.optJSONObject(i) ?: continue
                runCatching {
                    val env = if (pairing.accountUserId != null) {
                        CloudCrypto.Envelope.parse(JSONObject(row.getString("ciphertext")))
                    } else {
                        CloudCrypto.Envelope(row.getString("id"), row.getString("message_type"), row.getLong("seq"), row.getLong("sent_at"), row.getString("nonce"), row.getString("ciphertext"))
                    }
                    acceptEnvelope(env, row.getString("id"))
                }
            }
        }

        private fun acceptEnvelope(envelope: CloudCrypto.Envelope, queueId: String?) {
            try {
                if (queueId != null) delete(pairing, session, queueId)
                val plain = codec.open(envelope)
                codec.counters().let { onCounters(it.first, it.second) }
                when (envelope.kind) {
                    "data" -> {
                        incoming.put((plain + "\n").toByteArray(Charsets.UTF_8))
                        sendEnvelope("ack", envelope.id)
                    }
                    "ack" -> Unit
                }
            } catch (_: Exception) {
                // Realtime and queue can contain the same id. Old sequence numbers and unauthenticated frames are dropped.
            }
        }

        private fun sendEnvelope(kind: String, plain: String) {
            val envelope = codec.seal(plain, kind)
            codec.counters().let { onCounters(it.first, it.second) }
            insert(pairing, session, linkId, deviceId, peerId, envelope)
            ws.sendText(JSONObject().put("topic", topic).put("event", "broadcast")
                .put("payload", JSONObject().put("type", "broadcast").put("event", "macha_envelope").put("payload", JSONObject().put("envelope", envelope.json())))
                .put("ref", "msg-${ref++}").put("join_ref", "join-1").toString())
        }

        private val input = object : InputStream() {
            override fun read(): Int { val b = ByteArray(1); return if (read(b, 0, 1) < 0) -1 else b[0].toInt() and 0xff }
            override fun read(b: ByteArray, off: Int, len: Int): Int {
                if (len == 0) return 0
                if (pendingAt >= pending.size) {
                    val next = if (readTimeoutMs > 0) incoming.poll(readTimeoutMs.toLong(), TimeUnit.MILLISECONDS) ?: throw SocketTimeoutException("read timed out") else incoming.take()
                    if (next === END) { incoming.offer(END); return -1 }
                    pending = next; pendingAt = 0
                }
                val n = minOf(len, pending.size - pendingAt)
                System.arraycopy(pending, pendingAt, b, off, n); pendingAt += n
                return n
            }
        }
        private val output = object : OutputStream() {
            private val line = ByteArrayOutputStream()
            @Synchronized override fun write(b: Int) = write(byteArrayOf(b.toByte()), 0, 1)
            @Synchronized override fun write(b: ByteArray, off: Int, len: Int) {
                if (closed) throw IOException("Cloud connection is closed")
                for (i in off until off + len) {
                    if (b[i] == '\n'.code.toByte()) {
                        val text = line.toByteArray().toString(Charsets.UTF_8); line.reset()
                        try { sendEnvelope("data", text) } catch (e: Exception) { throw IOException("Could not send through Supabase", e) }
                    } else {
                        if (line.size() >= Protocol.MAX_LINE_BYTES) throw IOException("line too long")
                        line.write(b[i].toInt())
                    }
                }
            }
        }

        override fun getInputStream(): InputStream = input
        override fun getOutputStream(): OutputStream = output
        override fun setSoTimeout(timeout: Int) { readTimeoutMs = timeout }
        override fun getSoTimeout(): Int = readTimeoutMs
        override fun setTcpNoDelay(on: Boolean) {}
        override fun isConnected(): Boolean = !closed
        override fun isClosed(): Boolean = closed
        @Synchronized override fun close() {
            if (closed) return
            closed = true; runCatching { ws.close() }; codec.wipe(); incoming.offer(END)
        }
        private companion object { val END = ByteArray(0) }
    }
}

/** Cloud route used by the existing LinkClient; local TLS remains its preferred route. */
class SupabaseCloudConnector(
    pairing: SupabasePairing,
    private val onCounters: (Long, Long) -> Unit = { _, _ -> },
    private val sessionProvider: (() -> CloudSession)? = null,
    private val onRefresh: (String) -> Unit = {},
) : Connector {
    @Volatile private var current = pairing
    override fun connect(timeoutMs: Int): Socket = SupabaseCloud.open(current, timeoutMs,
        onRefresh = { token -> current = current.copy(refreshToken = token); onRefresh(token) },
        onCounters = { sent, received -> current = current.copy(sendSeq = sent, receiveSeq = received); onCounters(sent, received) },
        sessionProvider = sessionProvider,
    )
}
