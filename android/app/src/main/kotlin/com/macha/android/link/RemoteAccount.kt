package com.macha.android.link

import android.app.Activity
import android.content.Intent
import android.net.Uri
import com.macha.android.app.RemoteDeviceSession
import com.macha.android.app.SecureStore
import com.macha.android.BuildConfig
import org.json.JSONArray
import org.json.JSONObject
import java.io.ByteArrayOutputStream
import java.io.IOException
import java.net.HttpURLConnection
import java.net.URL
import java.security.MessageDigest
import java.security.SecureRandom
import java.util.Base64
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

data class RemoteDevice(
    val id: String,
    val type: String,
    val name: String,
    val publicKey: String,
    val status: String,
    val lastSeen: String?,
)

class RemoteSessionExpiredException : IOException("Your Google sign-in expired. Sign in again to reconnect this device.")

/** Browser-based Supabase OAuth PKCE and account-scoped device RPCs for Remote mode. */
class RemoteAccount(private val store: SecureStore) {
    val configured: Boolean
        get() = runCatching { requireConfigured(); true }.getOrDefault(false)

    suspend fun beginSignIn(activity: Activity) = withContext(Dispatchers.IO) {
        val config = requireConfigured()
        val settingsConnection = (URL("${config.url}/auth/v1/settings").openConnection() as HttpURLConnection).apply {
            requestMethod = "GET"
            connectTimeout = 15_000
            readTimeout = 15_000
            setRequestProperty("apikey", config.publishableKey)
        }
        val settings = try {
            val bytes = responseBytes(settingsConnection)
            JSONObject(String(bytes, Charsets.UTF_8))
        } finally { settingsConnection.disconnect() }
        if (settings.optJSONObject("external")?.optBoolean("google", false) != true) {
            throw IOException("Google sign-in is unavailable right now. Try again later.")
        }
        val verifier = Base64.getUrlEncoder().withoutPadding().encodeToString(ByteArray(32).also(SecureRandom()::nextBytes))
        val state = Base64.getUrlEncoder().withoutPadding().encodeToString(ByteArray(24).also(SecureRandom()::nextBytes))
        val challenge = Base64.getUrlEncoder().withoutPadding().encodeToString(MessageDigest.getInstance("SHA-256").digest(verifier.toByteArray(Charsets.US_ASCII)))
        store.savePendingRemoteSignIn(verifier, state)
        val redirectTo = Uri.parse("macha://remote-auth/callback").buildUpon().appendQueryParameter("state", state).build().toString()
        val authUrl = Uri.parse("${config.url}/auth/v1/authorize").buildUpon()
            .appendQueryParameter("provider", "google")
            .appendQueryParameter("apikey", config.publishableKey)
            .appendQueryParameter("redirect_to", redirectTo)
            .appendQueryParameter("code_challenge", challenge)
            .appendQueryParameter("code_challenge_method", "s256")
            .appendQueryParameter("query_params", JSONObject().put("prompt", "select_account").toString())
            .build()
        withContext(Dispatchers.Main) {
            try {
                activity.startActivity(Intent(Intent.ACTION_VIEW, authUrl))
            } catch (e: Exception) {
                store.clearPendingRemoteSignIn()
                throw IOException("Could not open a browser for Google sign-in.", e)
            }
        }
    }

    suspend fun completeSignIn(uri: Uri): RemoteDeviceSession = withContext(Dispatchers.IO) {
        val pending = store.takePendingRemoteSignIn() ?: throw IOException("Google sign-in expired or was cancelled. Try again.")
        if (uri.getQueryParameter("state") != pending.state) throw IOException("Google sign-in could not be verified. Try again.")
        val error = uri.getQueryParameter("error_description") ?: uri.getQueryParameter("error")
        if (!error.isNullOrBlank()) throw IOException("Google sign-in was cancelled or failed.")
        val code = uri.getQueryParameter("code")?.takeIf(String::isNotBlank) ?: throw IOException("Supabase did not return an authorization code.")
        val body = request("POST", "${baseUrl()}/auth/v1/token?grant_type=pkce", JSONObject()
            .put("auth_code", code).put("code_verifier", pending.verifier), null)
        val auth = parseAuth(body)
        // Keep the verified Google session even if device registration is temporarily
        // unavailable (for example, while the developer is applying the SQL migrations).
        store.saveRemoteSession(auth.refreshToken, auth.userId, auth.email)
        val private = store.loadRemoteIdentity() ?: CloudCrypto.generateIdentity().privateEncoded().also(store::saveRemoteIdentity)
        val identity = CloudCrypto.identityFromPrivate(private)
        val device = registerDevice(auth.accessToken, auth.userId, identity.publicEncoded())
        store.saveRemoteDevice(device.id, device.status)
        device
    }

    fun refreshDevices(): List<RemoteDevice> {
        requireConfigured()
        val auth = refreshSession()
        val own = store.loadRemoteDevice()
        val private = store.loadRemoteIdentity() ?: CloudCrypto.generateIdentity().privateEncoded().also(store::saveRemoteIdentity)
        val identity = CloudCrypto.identityFromPrivate(private)
        var devices = loadDevices(auth.accessToken)
        val ownServer = own?.let { saved -> devices.firstOrNull { it.id == saved.id } }
        // If no other approved device remains, safely retry enrollment. The
        // backend keeps this pending when a trusted peer still exists.
        val hasOtherApproved = devices.any { it.id != own?.id && it.status == "approved" }
        if (ownServer == null || (ownServer.status in setOf("pending", "revoked") && !hasOtherApproved)) {
            val registered = registerDevice(auth.accessToken, auth.userId, identity.publicEncoded())
            store.saveRemoteDevice(registered.id, registered.status)
            devices = loadDevices(auth.accessToken)
        }
        val serverOwn = store.loadRemoteDevice()?.let { saved -> devices.firstOrNull { it.id == saved.id } }
        if (serverOwn != null) store.saveRemoteDevice(serverOwn.id, serverOwn.status)
        return devices
    }

    private fun loadDevices(accessToken: String): List<RemoteDevice> {
        val response = rpcValue("list_devices", JSONObject(), accessToken)
        val array = when (response) {
            is JSONArray -> response
            is JSONObject -> response.optJSONArray("list_devices") ?: JSONArray()
            else -> JSONArray()
        }
        return (0 until array.length()).mapNotNull { index ->
            val d = array.optJSONObject(index) ?: return@mapNotNull null
            val id = d.optString("id").takeIf(String::isNotBlank) ?: return@mapNotNull null
            RemoteDevice(id, d.optString("type"), d.optString("name"), d.optString("pubkey"), d.optString("status"), d.optString("last_seen").ifBlank { null })
        }
    }

    private fun registerCurrentDevice(): RemoteDeviceSession {
        val auth = refreshSession()
        val private = store.loadRemoteIdentity() ?: CloudCrypto.generateIdentity().privateEncoded().also(store::saveRemoteIdentity)
        val identity = CloudCrypto.identityFromPrivate(private)
        return registerDevice(auth.accessToken, auth.userId, identity.publicEncoded()).also {
            store.saveRemoteDevice(it.id, it.status)
        }
    }

    /** Build an in-memory tunnel pairing for an already-approved laptop on this Google account. */
    fun accountPairing(peer: RemoteDevice): PairingPayload {
        if (peer.type != "laptop" || peer.status != "approved") throw IOException("Choose an approved laptop to connect with Remote mode.")
        val own = store.loadRemoteDevice() ?: throw IOException("This phone is not registered for Remote mode.")
        if (own.status != "approved") throw IOException("This phone is waiting for approval.")
        val auth = refreshSession()
        val privateKey = store.loadRemoteIdentity() ?: throw IOException("The phone's secure device key is missing. Sign in again.")
        val identity = CloudCrypto.identityFromPrivate(privateKey)
        val linkId = stableAccountLinkId(auth.userId, own.id, peer.id)
        val sharedKey = CloudCrypto.derive(identity, peer.publicKey, linkId)
        val token = CloudCrypto.encode(sharedKey)
        sharedKey.fill(0)
        val config = requireConfigured()
        val counters = store.loadRemoteCloudCounters(linkId)
        val cloud = SupabasePairing(
            url = config.url,
            anonKey = config.publishableKey,
            pairingId = linkId,
            token = token,
            laptopPubKey = peer.publicKey,
            linkId = linkId,
            deviceId = own.id,
            laptopDeviceId = peer.id,
            phonePrivateKey = privateKey,
            refreshToken = auth.refreshToken,
            sendSeq = counters.first,
            receiveSeq = counters.second,
            accountUserId = auth.userId,
        )
        return PairingPayload("remote", 1, "0".repeat(64), token, peer.name, cloud = cloud, accountRemote = true)
    }

    private fun stableAccountLinkId(userId: String, firstDevice: String, secondDevice: String): String {
        val material = "macha-remote-link-v1\n$userId\n${listOf(firstDevice, secondDevice).sorted().joinToString("\n")}".toByteArray(Charsets.UTF_8)
        val bytes = MessageDigest.getInstance("SHA-256").digest(material).copyOf(16)
        bytes[6] = ((bytes[6].toInt() and 0x0f) or 0x50).toByte()
        bytes[8] = ((bytes[8].toInt() and 0x3f) or 0x80).toByte()
        val b = java.nio.ByteBuffer.wrap(bytes)
        return java.util.UUID(b.long, b.long).toString()
    }

    fun approve(deviceId: String, approverDeviceId: String) {
        val auth = refreshSession()
        val approved = rpcValue("approve_device", JSONObject().put("p_device_id", deviceId).put("p_approver_device_id", approverDeviceId), auth.accessToken) as? Boolean
            ?: throw IOException("Supabase returned an unexpected approval response.")
        if (!approved) throw IOException("Device approval was not accepted. Refresh the device list and try again.")
    }

    fun revoke(deviceId: String) {
        val auth = refreshSession()
        val revoked = rpcValue("revoke_device", JSONObject().put("p_device_id", deviceId), auth.accessToken) as? Boolean
            ?: throw IOException("Supabase returned an unexpected device response.")
        if (!revoked) throw IOException("The device could not be revoked. Refresh the device list and try again.")
        if (store.loadRemoteDevice()?.id == deviceId) store.saveRemoteDevice(deviceId, "revoked")
    }

    fun deleteAccount() {
        val auth = refreshSession()
        rpc("delete_my_account_data", JSONObject(), auth.accessToken)
        store.clearRemoteAccount()
    }

    fun signOut() {
        // Authentication and device trust are separate controls. Signing out
        // must not revoke the device, or the user can strand every device with
        // no approved installation left to approve a replacement.
        val auth = runCatching { refreshSession() }.getOrNull()
        if (auth != null) runCatching { request("POST", "${baseUrl()}/auth/v1/logout", JSONObject(), auth.accessToken) }
        store.clearRemoteSession()
    }

    private data class Auth(val accessToken: String, val refreshToken: String, val userId: String, val email: String)

    /** Refresh through the same serialized path used by device polling and tunnel setup. */
    fun refreshCloudSession(): CloudSession = refreshSession().let {
        CloudSession(it.accessToken, it.refreshToken, it.userId, it.email)
    }

    @Synchronized
    private fun refreshSession(): Auth {
        val saved = store.loadRemoteSession() ?: throw IOException("Sign in with Google to use Remote mode.")
        val body = request("POST", "${baseUrl()}/auth/v1/token?grant_type=refresh_token", JSONObject().put("refresh_token", saved.refreshToken), null)
        val auth = parseAuth(body)
        if (auth.userId != saved.userId) throw IOException("The Google account changed. Sign in again with the same account on both devices.")
        store.saveRemoteSession(auth.refreshToken, auth.userId, auth.email)
        return auth
    }

    private fun registerDevice(access: String, userId: String, publicKey: String): RemoteDeviceSession {
        val name = android.os.Build.MODEL?.take(80)?.ifBlank { "Android phone" } ?: "Android phone"
        val result = rpc("register_device", JSONObject().put("p_type", "phone").put("p_name", name).put("p_pubkey", publicKey), access)
        val id = result.optString("device_id").takeIf(String::isNotBlank) ?: throw IOException("Supabase did not return the registered device ID.")
        if (userId.isBlank()) throw IOException("Google account ID is missing.")
        return RemoteDeviceSession(id, result.optString("status", "pending"))
    }

    private fun rpc(name: String, args: JSONObject, access: String): JSONObject =
        rpcValue(name, args, access) as? JSONObject ?: throw IOException("Supabase returned an unexpected response for $name.")

    private fun rpcValue(name: String, args: JSONObject, access: String): Any {
        val connection = open("${baseUrl()}/rest/v1/rpc/$name", "POST", access)
        try {
            connection.doOutput = true
            connection.setRequestProperty("Content-Type", "application/json")
            connection.outputStream.use { it.write(args.toString().toByteArray(Charsets.UTF_8)) }
            val bytes = responseBytes(connection)
            if (bytes.isEmpty()) return JSONObject()
            return org.json.JSONTokener(String(bytes, Charsets.UTF_8)).nextValue()
        } finally { connection.disconnect() }
    }

    private fun request(method: String, endpoint: String, body: JSONObject, access: String?): JSONObject {
        val connection = open(endpoint, method, access)
        try {
            connection.doOutput = true
            connection.setRequestProperty("Content-Type", "application/json")
            connection.outputStream.use { it.write(body.toString().toByteArray(Charsets.UTF_8)) }
            val bytes = responseBytes(connection)
            return if (bytes.isEmpty()) JSONObject() else JSONObject(String(bytes, Charsets.UTF_8))
        } finally { connection.disconnect() }
    }

    private fun open(endpoint: String, method: String, access: String?): HttpURLConnection {
        val config = requireConfigured()
        val c = (URL(endpoint).openConnection() as HttpURLConnection).apply {
            requestMethod = method
            connectTimeout = 15_000
            readTimeout = 15_000
            setRequestProperty("apikey", config.publishableKey)
            if (access != null) setRequestProperty("Authorization", "Bearer $access")
        }
        return c
    }

    private fun responseBytes(c: HttpURLConnection): ByteArray {
        val code = c.responseCode
        val stream = if (code in 200..299) c.inputStream else c.errorStream
        val bytes = stream?.use { input ->
            val output = ByteArrayOutputStream()
            val buffer = ByteArray(8192)
            while (true) {
                val n = input.read(buffer)
                if (n < 0) break
                if (output.size() + n > 1_000_000) throw IOException("Supabase response is too large.")
                output.write(buffer, 0, n)
            }
            output.toByteArray()
        } ?: ByteArray(0)
        if (code !in 200..299) {
            val error = runCatching { JSONObject(String(bytes, Charsets.UTF_8)) }.getOrNull()
            val message = error?.optString("msg")?.ifBlank { error.optString("message") }.orEmpty()
            val errorCode = error?.optString("error_code")?.ifBlank { error.optString("error") }.orEmpty()
            val isRefreshRequest = c.url.path == "/auth/v1/token" && c.url.query?.contains("grant_type=refresh_token") == true
            if (isRefreshRequest && (errorCode.equals("refresh_token_not_found", true) ||
                    message.contains("invalid refresh token", true) || message.contains("refresh token not found", true))) {
                throw RemoteSessionExpiredException()
            }
            throw IOException("Supabase returned HTTP $code${if (message.isBlank()) "" else ": ${message.take(180)}"}")
        }
        return bytes
    }

    private fun parseAuth(body: JSONObject): Auth {
        val user = body.optJSONObject("user") ?: throw IOException("Supabase Auth did not return a user.")
        val provider = user.optJSONObject("app_metadata")?.optString("provider")
        val email = user.optString("email")
        if (provider != "google" || email.isBlank() || user.optString("email_confirmed_at").isBlank()) {
            throw IOException("Use a verified Google account for Remote mode.")
        }
        return Auth(
            body.optString("access_token").takeIf(String::isNotBlank) ?: throw IOException("Supabase did not return an access token."),
            body.optString("refresh_token").takeIf(String::isNotBlank) ?: throw IOException("Supabase did not return a refresh token."),
            user.optString("id").takeIf(String::isNotBlank) ?: throw IOException("Supabase did not return an account ID."),
            email,
        )
    }

    private fun baseUrl(): String = requireConfigured().url

    private fun requireConfigured(): RemoteProjectConfig {
        val config = RemoteProjectConfig(BuildConfig.SUPABASE_URL.trimEnd('/'), BuildConfig.SUPABASE_ANON_KEY)
        if (config.url.isBlank() || config.publishableKey.isBlank()) throw IOException("Remote sync is unavailable in this build. Contact the Macha developer.")
        validateProjectUrl(config.url)
        validateClientKey(config.publishableKey)
        return config
    }

    private fun validateProjectUrl(value: String) {
        if (!value.matches(Regex("https://[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\\.supabase\\.co"))) {
            throw IOException("Remote sync is not configured correctly in this build.")
        }
    }

    private fun validateClientKey(value: String) {
        if (value.startsWith("sb_publishable_")) {
            if (value.removePrefix("sb_publishable_").length <= 8) throw IOException("Remote sync is not configured correctly in this build.")
            return
        }
        if (value.startsWith("sb_secret_") || value.equals("service_role", true)) throw IOException("Remote sync is not configured correctly in this build.")
        val claims = value.split('.').getOrNull(1) ?: throw IOException("Remote sync is not configured correctly in this build.")
        val payload = runCatching { JSONObject(String(Base64.getUrlDecoder().decode(claims), Charsets.UTF_8)) }.getOrNull()
        if (payload?.optString("role") != "anon") throw IOException("Remote sync is not configured correctly in this build.")
    }
}

private data class RemoteProjectConfig(val url: String, val publishableKey: String)
