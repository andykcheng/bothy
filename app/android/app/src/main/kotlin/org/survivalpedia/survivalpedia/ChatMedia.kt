package org.survivalpedia.survivalpedia

import android.Manifest
import android.app.Activity
import android.content.ClipData
import android.content.ContentProvider
import android.content.ContentValues
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.database.Cursor
import android.database.MatrixCursor
import android.net.Uri
import android.os.ParcelFileDescriptor
import android.provider.MediaStore
import android.provider.OpenableColumns
import io.flutter.plugin.common.MethodChannel
import java.io.File
import java.io.FileNotFoundException

/**
 * What the chat page (a WebView, lib/share.dart) needs from Android: a photo from the gallery or the
 * camera (or a sound file) for its file input, and the location and microphone permissions for "My
 * location" and voice messages. The page itself does the rest.
 */
object ChatMedia {
    private const val REQ_PICK = 61
    private const val REQ_PERM = 62
    private var picking: MethodChannel.Result? = null
    private var asking: MethodChannel.Result? = null
    private var startedAt = 0L

    /** accept: "image" or "audio". Answers with a list of content:// URIs (empty when cancelled). */
    fun pick(a: Activity, accept: String, result: MethodChannel.Result) {
        if (picking != null) {
            result.success(emptyList<String>())
            return
        }
        val image = accept != "audio"
        val get = Intent(Intent.ACTION_GET_CONTENT).setType(if (image) "image/*" else "audio/*").addCategory(Intent.CATEGORY_OPENABLE)
        val chooser = Intent.createChooser(get, if (image) "Photo" else "Voice message")
        if (image) {
            val out = PhotoProvider.reset(a)
            val shot = Intent(MediaStore.ACTION_IMAGE_CAPTURE)
                .putExtra(MediaStore.EXTRA_OUTPUT, out)
                .addFlags(Intent.FLAG_GRANT_WRITE_URI_PERMISSION or Intent.FLAG_GRANT_READ_URI_PERMISSION)
            shot.clipData = ClipData.newRawUri("photo", out)
            if (shot.resolveActivity(a.packageManager) != null) chooser.putExtra(Intent.EXTRA_INITIAL_INTENTS, arrayOf(shot))
        } else {
            val rec = Intent(MediaStore.Audio.Media.RECORD_SOUND_ACTION)
            if (rec.resolveActivity(a.packageManager) != null) chooser.putExtra(Intent.EXTRA_INITIAL_INTENTS, arrayOf(rec))
        }
        picking = result
        startedAt = System.currentTimeMillis()
        try {
            @Suppress("DEPRECATION")
            a.startActivityForResult(chooser, REQ_PICK)
        } catch (e: Exception) {
            picking = null
            result.error("none", "No app on this phone can pick a file", null)
        }
    }

    /** From MainActivity.onActivityResult; false when the request isn't ours. */
    fun onResult(a: Activity, req: Int, code: Int, data: Intent?): Boolean {
        if (req != REQ_PICK) return false
        val r = picking ?: return true
        picking = null
        val got = data?.data
        val shot = PhotoProvider.file(a)
        r.success(
            when {
                code != Activity.RESULT_OK -> emptyList()
                got != null -> listOf(got.toString())
                shot.length() > 0 && shot.lastModified() >= startedAt - 2000 -> listOf(PhotoProvider.uri(a).toString())
                else -> emptyList()
            },
        )
        return true
    }

    /** kind: "location" or "mic". Answers true when granted. */
    fun ask(a: Activity, kind: String, result: MethodChannel.Result) {
        val perms = if (kind == "mic") arrayOf(Manifest.permission.RECORD_AUDIO)
            else arrayOf(Manifest.permission.ACCESS_FINE_LOCATION, Manifest.permission.ACCESS_COARSE_LOCATION)
        if (perms.all { a.checkSelfPermission(it) == PackageManager.PERMISSION_GRANTED }) {
            result.success(true)
            return
        }
        if (asking != null) {
            result.success(false)
            return
        }
        asking = result
        a.requestPermissions(perms, REQ_PERM)
    }

    /** From MainActivity.onRequestPermissionsResult; false when the request isn't ours. */
    fun onPermission(req: Int, results: IntArray): Boolean {
        if (req != REQ_PERM) return false
        val r = asking ?: return true
        asking = null
        // location counts as granted when at least the approximate one is
        r.success(results.any { it == PackageManager.PERMISSION_GRANTED })
        return true
    }
}

/** One file the camera app writes a new photo into, and the chat page then reads. */
class PhotoProvider : ContentProvider() {
    companion object {
        fun file(c: Context) = File(c.cacheDir, "chat-camera.jpg")
        fun uri(c: Context): Uri = Uri.Builder().scheme("content").authority("${c.packageName}.photos").appendPath("camera.jpg").build()
        fun reset(c: Context): Uri {
            file(c).delete()
            return uri(c)
        }
    }

    private fun f(uri: Uri): File {
        if (uri.lastPathSegment != "camera.jpg") throw FileNotFoundException(uri.toString())
        return file(context!!)
    }

    override fun onCreate() = true
    override fun getType(uri: Uri) = "image/jpeg"

    override fun openFile(uri: Uri, mode: String): ParcelFileDescriptor =
        ParcelFileDescriptor.open(f(uri), ParcelFileDescriptor.parseMode(mode))

    override fun query(uri: Uri, projection: Array<out String>?, selection: String?, args: Array<out String>?, sort: String?): Cursor {
        val file = f(uri)
        val cols = projection ?: arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE)
        return MatrixCursor(cols, 1).apply {
            addRow(cols.map { if (it == OpenableColumns.SIZE) file.length() else if (it == OpenableColumns.DISPLAY_NAME) "photo.jpg" else null })
        }
    }

    override fun insert(uri: Uri, values: ContentValues?): Uri? = throw UnsupportedOperationException()
    override fun delete(uri: Uri, selection: String?, args: Array<out String>?) = 0
    override fun update(uri: Uri, values: ContentValues?, selection: String?, args: Array<out String>?) = 0
}
