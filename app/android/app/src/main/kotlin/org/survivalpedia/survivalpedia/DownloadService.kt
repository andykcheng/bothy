package org.survivalpedia.survivalpedia

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
import android.graphics.drawable.Icon
import android.net.wifi.WifiManager
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.PowerManager
import io.flutter.plugin.common.EventChannel
import org.json.JSONObject
import java.io.File
import java.io.FileOutputStream
import java.io.IOException
import java.net.HttpURLConnection
import java.net.URL

/** Events for the Dart side (downloads.dart, AndroidFetcher), delivered on the main thread. Dropped while the app is closed. */
object DownloadEvents {
    @Volatile var sink: EventChannel.EventSink? = null
    private val main = Handler(Looper.getMainLooper())
    fun emit(m: Map<String, Any?>) {
        main.post { sink?.success(m) }
    }
}

/**
 * Foreground service that downloads content files with a progress notification, so big downloads keep
 * going with the screen off, the app in the background or swiped away. It only fetches bytes: each job
 * is a URL fetched into a `.part` file, resuming from the part's length with an HTTP Range request.
 * Checking hashes and moving files into place is done by the app (downloads.dart), also for parts
 * finished while it was closed. Network errors are retried with backoff (5 s doubling to 60 s), and it
 * gives up after 15 minutes without progress.
 */
class DownloadService : Service() {
    companion object {
        const val ACTION_START = "org.survivalpedia.download.START"
        const val ACTION_PAUSE = "org.survivalpedia.download.PAUSE"
        private const val CHANNEL = "downloads"
        private const val NOTE_ID = 1
        private const val DONE_ID = 2
        @Volatile var instance: DownloadService? = null
    }

    private class Job(val url: String, val part: File, val size: Long, val label: String)
    private class Stopped : Exception()
    private class Fatal(msg: String) : Exception(msg)

    @Volatile private var generation = 0
    private var worker: Thread? = null
    private var wake: PowerManager.WakeLock? = null
    private var wifi: WifiManager.WifiLock? = null
    private var root: File? = null
    private var lastEvent = 0L
    private var lastNote = 0L

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        instance = this
        val nm = getSystemService(NotificationManager::class.java)
        nm.createNotificationChannel(NotificationChannel(CHANNEL, "Downloads", NotificationManager.IMPORTANCE_LOW))
    }

    override fun onDestroy() {
        instance = null
        releaseLocks()
        super.onDestroy()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_START -> {
                val path = intent.getStringExtra("jobs")
                // startForegroundService() needs startForeground() soon, whatever happens next
                goForeground(note("Preparing download", "", 0, 0))
                if (path == null) halt(null) else begin(File(path))
            }
            ACTION_PAUSE -> halt(null, paused = true)
            else -> {
                goForeground(note("Download", "", 0, 0))
                halt(null)
            }
        }
        return START_NOT_STICKY
    }

    // Android 15+: data-sync services may run 6 hours a day. The app resumes the queue when it next opens.
    override fun onTimeout(startId: Int, fgsType: Int) {
        halt("Android paused the download after 6 hours in the background. Tap Resume to carry on.")
    }

    private fun goForeground(n: Notification) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            startForeground(NOTE_ID, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
        } else {
            startForeground(NOTE_ID, n)
        }
    }

    private fun begin(jobsFile: File) {
        root = jobsFile.parentFile
        val old = worker
        val gen = ++generation
        acquireLocks()
        worker = Thread {
            old?.join() // the previous run, told to stop by the generation bump, must be done with its file
            run(jobsFile, gen)
        }.also { it.start() }
    }

    /** Stops the current run. `paused`: from the notification, remembered until the app opens. */
    fun halt(reason: String?, paused: Boolean = false) {
        generation++
        if (paused) root?.let { File(it, "downloads.paused").writeText("1") }
        DownloadEvents.emit(mapOf("type" to "stopped", "reason" to reason))
        finish()
    }

    private fun finish() {
        releaseLocks()
        stopForeground(STOP_FOREGROUND_REMOVE)
        stopSelf()
    }

    private fun run(jobsFile: File, gen: Int) {
        try {
            val j = JSONObject(jobsFile.readText())
            val title = j.optString("title", "Bothy")
            val arr = j.getJSONArray("jobs")
            val jobs = (0 until arr.length()).map {
                val o = arr.getJSONObject(it)
                Job(o.getString("url"), File(o.getString("part")), o.getLong("size"), o.optString("label", ""))
            }
            val total = jobs.sumOf { it.size }
            var before = 0L
            for ((i, job) in jobs.withIndex()) {
                fetch(job, gen, before, total, title)
                before += job.size
                DownloadEvents.emit(mapOf("type" to "file", "index" to i))
            }
            if (gen != generation) return
            progress(total, total, "", title, null, force = true)
            if (DownloadEvents.sink == null) {
                // the app is closed: it verifies and installs the files when it next opens
                getSystemService(NotificationManager::class.java).notify(
                    DONE_ID,
                    Notification.Builder(this, CHANNEL)
                        .setSmallIcon(android.R.drawable.stat_sys_download_done)
                        .setContentTitle("Download complete")
                        .setContentText("Open Bothy to finish installing")
                        .setContentIntent(openApp())
                        .setAutoCancel(true)
                        .build(),
                )
            }
            DownloadEvents.emit(mapOf("type" to "done"))
            Handler(Looper.getMainLooper()).post { if (gen == generation) finish() }
        } catch (e: Stopped) {
            // halted, or superseded by a newer start
        } catch (e: Exception) {
            if (gen == generation) {
                DownloadEvents.emit(mapOf("type" to "error", "message" to (e.message ?: e.toString())))
                Handler(Looper.getMainLooper()).post { if (gen == generation) finish() }
            }
        }
    }

    private fun check(gen: Int) {
        if (gen != generation) throw Stopped()
    }

    private fun fetch(job: Job, gen: Int, before: Long, total: Long, title: String) {
        job.part.parentFile?.mkdirs()
        var delay = 5L
        var lastGood = System.currentTimeMillis()
        while (true) {
            check(gen)
            var have = if (job.part.exists()) job.part.length() else 0L
            if (have > job.size) {
                job.part.delete()
                have = 0L
            }
            if (have == job.size) return
            var conn: HttpURLConnection? = null
            try {
                conn = (URL(job.url).openConnection() as HttpURLConnection).apply {
                    connectTimeout = 30_000
                    readTimeout = 60_000
                    if (have > 0) setRequestProperty("Range", "bytes=$have-")
                }
                val code = conn.responseCode
                val append = when {
                    code == 206 -> true
                    code == 200 -> { have = 0L; false } // the server ignored the range: start over
                    code == 416 -> { job.part.delete(); throw IOException("HTTP 416") }
                    code >= 500 -> throw IOException("HTTP $code")
                    else -> throw Fatal("HTTP $code for ${job.url}")
                }
                conn.inputStream.use { input ->
                    FileOutputStream(job.part, append).use { out ->
                        val buf = ByteArray(256 * 1024)
                        while (true) {
                            check(gen)
                            val n = input.read(buf)
                            if (n < 0) break
                            out.write(buf, 0, n)
                            have += n
                            lastGood = System.currentTimeMillis()
                            delay = 5L
                            progress(before + have, total, job.label, title, null)
                        }
                    }
                }
                if (have < job.size) throw IOException("connection closed early")
            } catch (e: IOException) {
                if (System.currentTimeMillis() - lastGood > 15 * 60_000L) {
                    throw Fatal("Lost the hub: ${e.message}. Check the Wi-Fi and tap Resume.")
                }
                progress(before + have, total, job.label, title, "Connection lost, retrying in $delay s", force = true)
                val until = System.currentTimeMillis() + delay * 1000
                while (System.currentTimeMillis() < until) {
                    check(gen)
                    Thread.sleep(250)
                }
                delay = minOf(delay * 2, 60L)
            } finally {
                conn?.disconnect()
            }
        }
    }

    private fun progress(bytes: Long, total: Long, label: String, title: String, noteText: String?, force: Boolean = false) {
        val now = System.currentTimeMillis()
        if (force || now - lastEvent >= 300) {
            lastEvent = now
            DownloadEvents.emit(mapOf("type" to "progress", "bytes" to bytes, "note" to noteText))
        }
        if (force || now - lastNote >= 1000) {
            lastNote = now
            val text = noteText ?: "${size(bytes)} of ${size(total)}"
            val permille = if (total > 0) (bytes * 1000 / total).toInt() else 0
            getSystemService(NotificationManager::class.java)
                .notify(NOTE_ID, note(if (label.isEmpty()) title else "Downloading $label", text, permille, 1000))
        }
    }

    private fun note(title: String, text: String, value: Int, max: Int): Notification {
        val pause = PendingIntent.getService(
            this, 1, Intent(this, DownloadService::class.java).setAction(ACTION_PAUSE),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        return Notification.Builder(this, CHANNEL)
            .setSmallIcon(android.R.drawable.stat_sys_download)
            .setContentTitle(title)
            .setContentText(text)
            .setProgress(max, value, max == 0)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setContentIntent(openApp())
            .addAction(
                Notification.Action.Builder(
                    Icon.createWithResource(this, android.R.drawable.ic_media_pause), "Pause", pause,
                ).build(),
            )
            .build()
    }

    private fun openApp(): PendingIntent? {
        val i = packageManager.getLaunchIntentForPackage(packageName) ?: return null
        return PendingIntent.getActivity(this, 0, i, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
    }

    private fun size(b: Long): String = when {
        b >= 1_000_000_000 -> String.format("%.2f GB", b / 1e9)
        b >= 1_000_000 -> String.format("%.0f MB", b / 1e6)
        else -> String.format("%.0f KB", b / 1e3)
    }

    @Suppress("DEPRECATION") // WIFI_MODE_FULL_HIGH_PERF: no-op on Android 14+, where the foreground service keeps Wi-Fi up
    private fun acquireLocks() {
        if (wake == null) {
            wake = getSystemService(PowerManager::class.java)
                .newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "survivalpedia:download")
                .apply { setReferenceCounted(false); acquire(7 * 3600_000L) }
        }
        if (wifi == null) {
            wifi = (applicationContext.getSystemService(WIFI_SERVICE) as WifiManager)
                .createWifiLock(WifiManager.WIFI_MODE_FULL_HIGH_PERF, "survivalpedia:download")
                .apply { setReferenceCounted(false); acquire() }
        }
    }

    private fun releaseLocks() {
        wake?.let { if (it.isHeld) it.release() }
        wake = null
        wifi?.let { if (it.isHeld) it.release() }
        wifi = null
    }
}
