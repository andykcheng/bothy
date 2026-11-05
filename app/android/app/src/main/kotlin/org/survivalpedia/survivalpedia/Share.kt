package org.survivalpedia.survivalpedia

import android.Manifest
import android.app.Activity
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.content.pm.ServiceInfo
import android.graphics.drawable.Icon
import android.location.LocationManager
import android.net.wifi.WifiManager
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.PowerManager
import android.provider.Settings
import io.flutter.plugin.common.MethodChannel

/**
 * Sharing the phone's server with people nearby (lib/share.dart). The Rust core listens on the network
 * (sp_share); this side starts a Wi-Fi network of the phone's own (Android's local-only hotspot, with a
 * name and password Android picks each time) and keeps the app running in the background with a
 * notification (ShareService), so the phone can sit on a power bank with its screen off.
 */
object Share {
    private const val REQ_PERM = 51
    var channel: MethodChannel? = null
    private var reservation: WifiManager.LocalOnlyHotspotReservation? = null
    private var pending: MethodChannel.Result? = null

    fun handle(a: Activity, method: String, result: MethodChannel.Result) {
        when (method) {
            "startHotspot" -> startHotspot(a, result)
            "stopHotspot" -> {
                stopHotspot()
                result.success(null)
            }
            "startService" -> {
                a.startForegroundService(Intent(a, ShareService::class.java).setAction(ShareService.ACTION_START))
                result.success(null)
            }
            "stopService" -> {
                a.stopService(Intent(a, ShareService::class.java))
                result.success(null)
            }
            "locationSettings" -> {
                a.startActivity(Intent(Settings.ACTION_LOCATION_SOURCE_SETTINGS))
                result.success(null)
            }
            "hotspotSettings" -> {
                // the system hotspot screen; not a public action everywhere, so fall back to wireless settings
                val tether = Intent().setClassName("com.android.settings", "com.android.settings.TetherSettings")
                try {
                    a.startActivity(tether)
                } catch (e: Exception) {
                    a.startActivity(Intent(Settings.ACTION_WIRELESS_SETTINGS))
                }
                result.success(null)
            }
            else -> result.notImplemented()
        }
    }

    private fun permission(): String =
        if (Build.VERSION.SDK_INT >= 33) Manifest.permission.NEARBY_WIFI_DEVICES else Manifest.permission.ACCESS_FINE_LOCATION

    private fun startHotspot(a: Activity, result: MethodChannel.Result) {
        if (pending != null) {
            result.error("busy", "Already starting", null)
            return
        }
        reservation?.let {
            result.success(creds(it))
            return
        }
        pending = result
        if (a.checkSelfPermission(permission()) != PackageManager.PERMISSION_GRANTED) {
            // Android 12 ignores a request for fine location that doesn't also ask for coarse
            val ask = if (Build.VERSION.SDK_INT >= 33) arrayOf(permission())
                else arrayOf(Manifest.permission.ACCESS_FINE_LOCATION, Manifest.permission.ACCESS_COARSE_LOCATION)
            a.requestPermissions(ask, REQ_PERM)
        } else {
            begin(a)
        }
    }

    /** From MainActivity.onRequestPermissionsResult; false when the request isn't ours. */
    fun onPermission(a: Activity, req: Int, granted: Boolean): Boolean {
        if (req != REQ_PERM) return false
        if (granted) {
            begin(a)
        } else {
            fail("permission", "Bothy needs permission to start a Wi-Fi network")
        }
        return true
    }

    private fun fail(code: String, message: String) {
        val r = pending ?: return
        pending = null
        r.error(code, message, null)
    }

    private fun begin(a: Activity) {
        // Android 8–12 only start a hotspot for apps while location is on
        if (Build.VERSION.SDK_INT < 33) {
            val lm = a.getSystemService(LocationManager::class.java)
            val on = if (Build.VERSION.SDK_INT >= 28) lm.isLocationEnabled else
                @Suppress("DEPRECATION") Settings.Secure.getInt(a.contentResolver, Settings.Secure.LOCATION_MODE, 0) != 0
            if (!on) {
                fail("location_off", "Turn on Location to start a Wi-Fi network (Android needs it; nothing is tracked)")
                return
            }
        }
        val wm = a.applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
        try {
            wm.startLocalOnlyHotspot(object : WifiManager.LocalOnlyHotspotCallback() {
                override fun onStarted(r: WifiManager.LocalOnlyHotspotReservation) {
                    reservation = r
                    val p = pending
                    pending = null
                    p?.success(creds(r))
                }

                override fun onStopped() {
                    reservation = null
                    channel?.invokeMethod("hotspotStopped", null)
                }

                override fun onFailed(reason: Int) {
                    fail(
                        "failed",
                        when (reason) {
                            ERROR_INCOMPATIBLE_MODE -> "Your phone's own hotspot is on. Turn it off, or choose \"My hotspot is already on\"."
                            ERROR_TETHERING_DISALLOWED -> "This phone does not allow hotspots (set by its owner or carrier)."
                            ERROR_NO_CHANNEL -> "No free Wi-Fi channel. Try again in a moment."
                            else -> "Android could not start the Wi-Fi network. If your own hotspot is on, use that instead."
                        },
                    )
                }
            }, Handler(Looper.getMainLooper()))
        } catch (e: Exception) {
            fail("failed", e.message ?: "Could not start the Wi-Fi network")
        }
    }

    @Suppress("DEPRECATION")
    private fun creds(r: WifiManager.LocalOnlyHotspotReservation): Map<String, Any?> {
        var ssid: String?
        var psk: String?
        if (Build.VERSION.SDK_INT >= 30) {
            val c = r.softApConfiguration
            ssid = c.ssid
            psk = c.passphrase
        } else {
            val c = r.wifiConfiguration
            ssid = c?.SSID
            psk = c?.preSharedKey
        }
        return mapOf("ssid" to ssid?.removeSurrounding("\""), "password" to psk?.removeSurrounding("\""))
    }

    fun stopHotspot() {
        reservation?.close()
        reservation = null
    }
}

/** Keeps the app (and so the server and the hotspot) running while it shares; its Stop button ends sharing. */
class ShareService : Service() {
    companion object {
        const val ACTION_START = "start"
        const val ACTION_STOP = "stop"
        private const val CHANNEL = "share"
        private const val NOTE_ID = 2
    }

    private var wake: PowerManager.WakeLock? = null
    private var wifi: WifiManager.WifiLock? = null

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        getSystemService(NotificationManager::class.java)
            .createNotificationChannel(NotificationChannel(CHANNEL, "Sharing nearby", NotificationManager.IMPORTANCE_LOW))
    }

    @Suppress("DEPRECATION")
    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val stop = PendingIntent.getService(
            this, 3, Intent(this, ShareService::class.java).setAction(ACTION_STOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val open = packageManager.getLaunchIntentForPackage(packageName)?.let {
            PendingIntent.getActivity(this, 0, it, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        }
        val n = Notification.Builder(this, CHANNEL)
            .setSmallIcon(android.R.drawable.stat_sys_upload)
            .setContentTitle("Sharing with people nearby")
            .setContentText("The library, books, maps, games and chat are open on Wi-Fi")
            .setOngoing(true)
            .setContentIntent(open)
            .addAction(Notification.Action.Builder(Icon.createWithResource(this, android.R.drawable.ic_menu_close_clear_cancel), "Stop", stop).build())
            .build()
        if (Build.VERSION.SDK_INT >= 29) {
            startForeground(NOTE_ID, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE)
        } else {
            startForeground(NOTE_ID, n)
        }
        if (intent?.action == ACTION_STOP) {
            Share.stopHotspot()
            Share.channel?.invokeMethod("stopRequested", null)
            stopSelf()
            return START_NOT_STICKY
        }
        if (wake == null) {
            wake = getSystemService(PowerManager::class.java)
                .newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "survivalpedia:share")
                .apply { setReferenceCounted(false); acquire() }
        }
        if (wifi == null) {
            wifi = (applicationContext.getSystemService(WIFI_SERVICE) as WifiManager)
                .createWifiLock(WifiManager.WIFI_MODE_FULL_HIGH_PERF, "survivalpedia:share")
                .apply { setReferenceCounted(false); acquire() }
        }
        return START_NOT_STICKY
    }

    override fun onDestroy() {
        wake?.release()
        wifi?.release()
        wake = null
        wifi = null
        super.onDestroy()
    }
}
