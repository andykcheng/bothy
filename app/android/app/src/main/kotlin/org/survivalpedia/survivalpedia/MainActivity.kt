package org.survivalpedia.survivalpedia

import android.Manifest
import android.content.ActivityNotFoundException
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.EventChannel
import io.flutter.plugin.common.MethodChannel
import java.io.File

class MainActivity : FlutterActivity() {
    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        val messenger = flutterEngine.dartExecutor.binaryMessenger
        // Background downloads (DownloadService.kt; Dart side in lib/downloads.dart)
        MethodChannel(messenger, "survivalpedia/downloads").setMethodCallHandler { call, result ->
            when (call.method) {
                "start" -> {
                    askNotifications()
                    val i = Intent(this, DownloadService::class.java)
                        .setAction(DownloadService.ACTION_START)
                        .putExtra("jobs", call.argument<String>("jobs"))
                    startForegroundService(i)
                    result.success(null)
                }
                "stop" -> {
                    val s = DownloadService.instance
                    if (s != null) s.halt(null) else DownloadEvents.emit(mapOf("type" to "stopped", "reason" to null))
                    result.success(null)
                }
                else -> result.notImplemented()
            }
        }
        // "Open in another app" for downloaded books (BookProvider.kt)
        MethodChannel(messenger, "survivalpedia/files").setMethodCallHandler { call, result ->
            when (call.method) {
                "openWith" -> try {
                    val uri = BookProvider.share(this, File(call.argument<String>("path")!!), call.argument<String>("name") ?: "book.epub")
                    val view = Intent(Intent.ACTION_VIEW)
                        .setDataAndType(uri, "application/epub+zip")
                        .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                    startActivity(Intent.createChooser(view, "Open with").addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION))
                    result.success(null)
                } catch (e: ActivityNotFoundException) {
                    result.error("none", "No app on this phone opens EPUB books", null)
                } catch (e: Exception) {
                    result.error("failed", e.message, null)
                }
                // Notes: open the phone app with a number filled in (ACTION_DIAL needs no permission)
                "dial" -> try {
                    startActivity(Intent(Intent.ACTION_DIAL, Uri.parse("tel:" + Uri.encode(call.argument<String>("number")!!))))
                    result.success(null)
                } catch (e: ActivityNotFoundException) {
                    result.error("none", "This device has no phone app", null)
                }
                // a link from the chat page another app should open (a geo: place)
                "openUri" -> try {
                    startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(call.argument<String>("uri")!!)))
                    result.success(null)
                } catch (e: ActivityNotFoundException) {
                    result.error("none", "No app on this phone opens this", null)
                }
                // this app's APK, which a sharing phone offers to phones nearby (/app.apk)
                "apkPath" -> result.success(applicationInfo.sourceDir)
                // the chat page's file input and permissions (ChatMedia.kt)
                "pickMedia" -> ChatMedia.pick(this, call.argument<String>("accept") ?: "image", result)
                "askPermission" -> ChatMedia.ask(this, call.argument<String>("kind")!!, result)
                // the user's own EPUBs (BookImport.kt)
                "importFolder", "importFiles" ->
                    BookImport.start(this, call.method == "importFolder", call.argument<String>("dest")!!, result)
                else -> result.notImplemented()
            }
        }
        // Sharing with people nearby (Share.kt; Dart side in lib/share.dart)
        val share = MethodChannel(messenger, "survivalpedia/share")
        Share.channel = share
        share.setMethodCallHandler { call, result -> Share.handle(this, call.method, result) }
        EventChannel(messenger, "survivalpedia/downloads/events").setStreamHandler(object : EventChannel.StreamHandler {
            override fun onListen(arguments: Any?, events: EventChannel.EventSink?) {
                DownloadEvents.sink = events
            }

            override fun onCancel(arguments: Any?) {
                DownloadEvents.sink = null
            }
        })
    }

    @Deprecated("startActivityForResult is what FlutterActivity offers")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        if (!BookImport.onResult(this, requestCode, resultCode, data) &&
            !ChatMedia.onResult(this, requestCode, resultCode, data)) super.onActivityResult(requestCode, resultCode, data)
    }

    override fun onRequestPermissionsResult(requestCode: Int, permissions: Array<out String>, grantResults: IntArray) {
        val granted = grantResults.isNotEmpty() && grantResults.all { it == PackageManager.PERMISSION_GRANTED }
        if (!Share.onPermission(this, requestCode, granted) &&
            !ChatMedia.onPermission(requestCode, grantResults)) super.onRequestPermissionsResult(requestCode, permissions, grantResults)
    }

    override fun cleanUpFlutterEngine(flutterEngine: FlutterEngine) {
        Share.channel = null
        DownloadEvents.sink = null
        super.cleanUpFlutterEngine(flutterEngine)
    }

    // The progress notification needs this on Android 13+; downloads work without it, just unseen.
    private fun askNotifications() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
            checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) {
            requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), 7)
        }
    }
}
