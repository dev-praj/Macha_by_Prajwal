package com.macha.android.app

import android.content.Context
import android.content.BroadcastReceiver
import android.content.Intent
import android.content.IntentFilter
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import androidx.core.content.ContextCompat

/**
 * Tells the model when Wi-Fi, cable, or this phone's hotspot changes and whether a local route may be available.
 * Registered only while a computer is paired; the system calls back on a change, nothing polls.
 */
class NetworkWatch(
    context: Context,
    private val onChange: (wifiUp: Boolean) -> Unit,
    /** A network became the default one (Wi-Fi joined, Wi-Fi to mobile data): the link may be on the old one and half dead. */
    private val onNewNetwork: () -> Unit = {},
) {
    private val app = context.applicationContext
    private val cm = app.getSystemService(ConnectivityManager::class.java)
    private var callback: ConnectivityManager.NetworkCallback? = null
    private var hotspotReceiver: BroadcastReceiver? = null
    @Volatile private var hotspotUp = false

    @Volatile var wifiUp: Boolean = isWifiUp()
        private set

    fun isWifiUp(): Boolean {
        return hotspotUp || networkCapabilities().any { caps ->
            caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) ||
                caps.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET)
        }
    }

    fun isCellular(): Boolean {
        return networkCapabilities().any { it.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR) }
    }

    /** A cellular default route can still reach a computer attached to this phone's Wi-Fi hotspot. */
    fun hasLocalNetwork(): Boolean = hotspotUp || networkCapabilities().any { caps ->
        caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) ||
            caps.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET) ||
            caps.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR)
    }

    @Suppress("DEPRECATION") // allNetworks is the only public API that exposes the underlying Wi-Fi while a VPN is active.
    private fun networkCapabilities(): List<NetworkCapabilities> = runCatching {
        cm.allNetworks.mapNotNull(cm::getNetworkCapabilities)
    }.getOrDefault(emptyList())

    private fun updateHotspot(up: Boolean) {
        if (hotspotUp == up) return
        hotspotUp = up
        changed()
        onNewNetwork()
    }

    @Synchronized
    fun start() {
        if (callback != null) return
        val cb = object : ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: Network) { changed(); onNewNetwork() }
            override fun onLost(network: Network) { changed() }
            override fun onCapabilitiesChanged(network: Network, caps: NetworkCapabilities) { changed() }
        }
        callback = cb
        runCatching { cm.registerDefaultNetworkCallback(cb) }

        // Android exposes this broadcast to regular apps, while TetheringManager's
        // public callback was only added in API 36. Keep the action/extra literals
        // here because the corresponding WifiManager constants are @SystemApi.
        val receiver = object : BroadcastReceiver() {
            override fun onReceive(context: Context, intent: Intent) {
                when (intent.action) {
                    WIFI_AP_STATE_CHANGED -> when (intent.getIntExtra(WIFI_STATE_EXTRA, -1)) {
                        WIFI_AP_STATE_ENABLED -> updateHotspot(true)
                        WIFI_AP_STATE_DISABLED, WIFI_AP_STATE_FAILED -> updateHotspot(false)
                    }
                    TETHER_STATE_CHANGED -> {
                        val active = intent.getStringArrayExtra(ACTIVE_TETHER_EXTRA)
                        if (active != null) updateHotspot(active.isNotEmpty())
                    }
                }
            }
        }
        hotspotReceiver = receiver
        val filter = IntentFilter().apply {
            addAction(WIFI_AP_STATE_CHANGED)
            addAction(TETHER_STATE_CHANGED)
        }
        runCatching {
            val sticky = ContextCompat.registerReceiver(app, receiver, filter, ContextCompat.RECEIVER_NOT_EXPORTED)
            if (sticky?.action == WIFI_AP_STATE_CHANGED) {
                when (sticky.getIntExtra(WIFI_STATE_EXTRA, -1)) {
                    WIFI_AP_STATE_ENABLED -> updateHotspot(true)
                    WIFI_AP_STATE_DISABLED, WIFI_AP_STATE_FAILED -> updateHotspot(false)
                }
            }
        }
    }

    @Synchronized
    fun stop() {
        callback?.let { runCatching { cm.unregisterNetworkCallback(it) } }
        callback = null
        hotspotReceiver?.let { runCatching { app.unregisterReceiver(it) } }
        hotspotReceiver = null
        hotspotUp = false
    }

    private fun changed() {
        val up = isWifiUp()
        wifiUp = up
        onChange(up)
    }

    private companion object {
        const val WIFI_AP_STATE_CHANGED = "android.net.wifi.WIFI_AP_STATE_CHANGED"
        const val WIFI_STATE_EXTRA = "wifi_state"
        const val WIFI_AP_STATE_ENABLED = 13
        const val WIFI_AP_STATE_DISABLED = 11
        const val WIFI_AP_STATE_FAILED = 14
        const val TETHER_STATE_CHANGED = "android.net.conn.TETHER_STATE_CHANGED"
        const val ACTIVE_TETHER_EXTRA = "activeArray"
    }
}
