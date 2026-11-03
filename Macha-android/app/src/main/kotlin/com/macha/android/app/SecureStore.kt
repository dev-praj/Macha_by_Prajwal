package com.macha.android.app

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import com.macha.android.link.PairingPayload
import com.macha.android.link.PairingCodec
import java.security.KeyStore
import org.json.JSONObject
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * Keeps the pairing (which holds the secret token) encrypted with an AES-GCM key that never
 * leaves the Android Keystore. Backups are off (allowBackup=false), so it stays on this device.
 */
class SecureStore(context: Context) {
    private val prefs = context.getSharedPreferences("macha_link", Context.MODE_PRIVATE)
    init { prefs.edit().remove(REMOTE_PROJECT).apply() } // discard credentials from the retired BYO-project screen

    private fun key(): SecretKey {
        val ks = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (ks.getKey(ALIAS, null) as? SecretKey)?.let { return it }
        return KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore").apply {
            init(
                KeyGenParameterSpec.Builder(ALIAS, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                    .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                    .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                    .setKeySize(256)
                    .build(),
            )
        }.generateKey()
    }

    fun savePairing(p: PairingPayload) {
        // K and the relay's access key are inside the same Keystore-encrypted blob as the token: never in clear on the disk.
        val plain = PairingCodec.encode(p)
        val c = Cipher.getInstance("AES/GCM/NoPadding").apply { init(Cipher.ENCRYPT_MODE, key()) }
        val ct = c.doFinal(plain.toByteArray(Charsets.UTF_8))
        prefs.edit().putString(PREF, Base64.encodeToString(c.iv + ct, Base64.NO_WRAP)).apply()
    }

    fun loadPairing(): PairingPayload? {
        val blob = prefs.getString(PREF, null) ?: return null
        return try {
            val raw = Base64.decode(blob, Base64.NO_WRAP)
            val c = Cipher.getInstance("AES/GCM/NoPadding").apply {
                init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(128, raw, 0, IV_BYTES))
            }
            PairingCodec.decode(String(c.doFinal(raw, IV_BYTES, raw.size - IV_BYTES), Charsets.UTF_8)) ?: error("unreadable pairing")
        } catch (_: Exception) {
            clearPairing() // unreadable (key lost, corrupted): start clean instead of crashing
            null
        }
    }

    fun clearPairing() = prefs.edit().remove(PREF).apply()

    fun saveGoogleSession(refresh: String, userId: String, email: String) = saveSecret(GOOGLE_SESSION, JSONObject()
        .put("refresh", refresh).put("userId", userId).put("email", email).toString())

    fun loadGoogleSession(): GoogleSession? = runCatching {
        val json = JSONObject(loadSecret(GOOGLE_SESSION) ?: return null)
        GoogleSession(json.getString("refresh"), json.getString("userId"), json.getString("email"))
    }.getOrNull()

    fun clearGoogleSession() = prefs.edit().remove(GOOGLE_SESSION).apply()

    fun saveRemoteSession(refresh: String, userId: String, email: String) = saveSecret(REMOTE_SESSION, JSONObject()
        .put("refresh", refresh).put("userId", userId).put("email", email).toString())

    fun loadRemoteSession(): RemoteGoogleSession? = runCatching {
        val json = JSONObject(loadSecret(REMOTE_SESSION) ?: return null)
        RemoteGoogleSession(json.getString("refresh"), json.getString("userId"), json.getString("email"))
    }.getOrNull()

    fun saveRemoteIdentity(privateKey: String) = saveSecret(REMOTE_IDENTITY, privateKey)
    fun loadRemoteIdentity(): String? = runCatching { loadSecret(REMOTE_IDENTITY) }.getOrNull()

    fun saveRemoteCloudCounters(linkId: String, sent: Long, received: Long) {
        val counters = runCatching { loadSecret(REMOTE_CLOUD_SEQUENCE)?.let(::JSONObject) }.getOrNull() ?: JSONObject()
        counters.put(linkId, JSONObject().put("sent", sent).put("received", received))
        saveSecret(REMOTE_CLOUD_SEQUENCE, counters.toString())
    }

    fun loadRemoteCloudCounters(linkId: String): Pair<Long, Long> = runCatching {
        val value = JSONObject(loadSecret(REMOTE_CLOUD_SEQUENCE) ?: return 0L to 0L).optJSONObject(linkId) ?: return 0L to 0L
        value.optLong("sent") to value.optLong("received")
    }.getOrDefault(0L to 0L)

    fun saveRemoteDevice(id: String, status: String) = saveSecret(REMOTE_DEVICE, JSONObject()
        .put("id", id).put("status", status).toString())

    fun loadRemoteDevice(): RemoteDeviceSession? = runCatching {
        val json = JSONObject(loadSecret(REMOTE_DEVICE) ?: return null)
        RemoteDeviceSession(json.getString("id"), json.getString("status"))
    }.getOrNull()

    fun clearRemoteAccount() {
        prefs.edit().remove(REMOTE_SESSION).remove(REMOTE_IDENTITY).remove(REMOTE_DEVICE).remove(REMOTE_CLOUD_SEQUENCE)
            .putBoolean(REMOTE_USE_ACCOUNT_LINK, false).apply()
    }

    /** Expired OAuth sessions can be replaced without rotating this installation's device identity. */
    fun clearRemoteSession() { prefs.edit().remove(REMOTE_SESSION).apply() }

    fun remoteSignInSkipped(): Boolean = prefs.getBoolean(REMOTE_SIGN_IN_SKIPPED, false)
    fun setRemoteSignInSkipped(skipped: Boolean) { prefs.edit().putBoolean(REMOTE_SIGN_IN_SKIPPED, skipped).apply() }
    fun remoteUseAccountLink(): Boolean = prefs.getBoolean(REMOTE_USE_ACCOUNT_LINK, false)
    fun setRemoteUseAccountLink(enabled: Boolean) { prefs.edit().putBoolean(REMOTE_USE_ACCOUNT_LINK, enabled).apply() }

    fun savePendingRemoteSignIn(verifier: String, state: String) = saveSecret(PENDING_REMOTE_SIGN_IN, JSONObject()
        .put("verifier", verifier).put("state", state).put("createdAt", System.currentTimeMillis()).toString())

    fun takePendingRemoteSignIn(): PendingRemoteSignIn? {
        val json = runCatching { loadSecret(PENDING_REMOTE_SIGN_IN)?.let(::JSONObject) }.getOrNull() ?: return null
        prefs.edit().remove(PENDING_REMOTE_SIGN_IN).apply()
        return runCatching {
            val createdAt = json.getLong("createdAt")
            if (System.currentTimeMillis() - createdAt > 3 * 60 * 1000) return null
            PendingRemoteSignIn(json.getString("verifier"), json.getString("state"))
        }.getOrNull()
    }

    fun clearPendingRemoteSignIn() { prefs.edit().remove(PENDING_REMOTE_SIGN_IN).apply() }
    fun hasPendingRemoteSignIn(): Boolean = prefs.contains(PENDING_REMOTE_SIGN_IN)

    fun savePendingGoogleSignIn(verifier: String, pairingLink: String) = saveSecret(PENDING_GOOGLE, JSONObject()
        .put("verifier", verifier).put("pairingLink", pairingLink).toString())

    fun takePendingGoogleSignIn(): PendingGoogleSignIn? {
        val json = runCatching { loadSecret(PENDING_GOOGLE)?.let(::JSONObject) }.getOrNull() ?: return null
        prefs.edit().remove(PENDING_GOOGLE).apply()
        return runCatching { PendingGoogleSignIn(json.getString("verifier"), json.getString("pairingLink")) }.getOrNull()
    }

    private fun saveSecret(name: String, value: String) {
        val c = Cipher.getInstance("AES/GCM/NoPadding").apply { init(Cipher.ENCRYPT_MODE, key()) }
        val ct = c.doFinal(value.toByteArray(Charsets.UTF_8))
        prefs.edit().putString(name, Base64.encodeToString(c.iv + ct, Base64.NO_WRAP)).apply()
    }

    private fun loadSecret(name: String): String? {
        val blob = prefs.getString(name, null) ?: return null
        val raw = Base64.decode(blob, Base64.NO_WRAP)
        val c = Cipher.getInstance("AES/GCM/NoPadding").apply {
            init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(128, raw, 0, IV_BYTES))
        }
        return String(c.doFinal(raw, IV_BYTES, raw.size - IV_BYTES), Charsets.UTF_8)
    }

    private companion object {
        const val ALIAS = "macha_pairing_key"
        const val PREF = "pairing"
        const val GOOGLE_SESSION = "google_session"
        const val PENDING_GOOGLE = "pending_google"
        const val REMOTE_SESSION = "remote_google_session"
        const val REMOTE_IDENTITY = "remote_device_identity"
        const val REMOTE_DEVICE = "remote_device_registration"
        const val REMOTE_CLOUD_SEQUENCE = "remote_cloud_sequence"
        const val REMOTE_PROJECT = "remote_project_configuration"
        const val REMOTE_SIGN_IN_SKIPPED = "remote_signin_skipped"
        const val REMOTE_USE_ACCOUNT_LINK = "remote_use_account_link"
        const val PENDING_REMOTE_SIGN_IN = "pending_remote_signin"
        const val IV_BYTES = 12
    }
}

data class GoogleSession(val refreshToken: String, val userId: String, val email: String)
data class RemoteGoogleSession(val refreshToken: String, val userId: String, val email: String)
data class RemoteDeviceSession(val id: String, val status: String)
data class PendingGoogleSignIn(val verifier: String, val pairingLink: String)
data class PendingRemoteSignIn(val verifier: String, val state: String)
