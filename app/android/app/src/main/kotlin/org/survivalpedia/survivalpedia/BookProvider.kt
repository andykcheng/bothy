package org.survivalpedia.survivalpedia

import android.content.ContentProvider
import android.content.ContentValues
import android.content.Context
import android.database.Cursor
import android.database.MatrixCursor
import android.net.Uri
import android.os.ParcelFileDescriptor
import android.provider.OpenableColumns
import java.io.File
import java.io.FileNotFoundException
import java.util.concurrent.ConcurrentHashMap

/**
 * Read-only access to a downloaded EPUB for another app ("Open in another app" in the Books tab).
 * `share()` registers a file under a random name and returns its content:// URI; the receiving app gets
 * a temporary read grant through the intent. Only files inside this app's own storage are shared.
 */
class BookProvider : ContentProvider() {
    companion object {
        private val files = ConcurrentHashMap<String, Pair<File, String>>() // key -> (file, display name)

        fun authority(c: Context) = "${c.packageName}.books"

        fun share(c: Context, file: File, name: String): Uri {
            val f = file.canonicalFile
            val roots = listOfNotNull(c.getExternalFilesDir(null), c.filesDir.parentFile).map { it.canonicalPath }
            require(roots.any { f.path.startsWith(it + File.separator) }) { "not an app file: $f" }
            require(f.isFile) { "missing: $f" }
            val key = java.util.UUID.randomUUID().toString() + ".epub"
            files[key] = f to name
            return Uri.Builder().scheme("content").authority(authority(c)).appendPath(key).build()
        }
    }

    private fun lookup(uri: Uri) = uri.lastPathSegment?.let { files[it] } ?: throw FileNotFoundException(uri.toString())

    override fun onCreate() = true

    override fun getType(uri: Uri) = "application/epub+zip"

    override fun openFile(uri: Uri, mode: String): ParcelFileDescriptor {
        if (mode != "r") throw SecurityException("read-only")
        return ParcelFileDescriptor.open(lookup(uri).first, ParcelFileDescriptor.MODE_READ_ONLY)
    }

    override fun query(uri: Uri, projection: Array<out String>?, selection: String?, args: Array<out String>?, sort: String?): Cursor {
        val (f, name) = lookup(uri)
        val cols = projection ?: arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE)
        return MatrixCursor(cols, 1).apply {
            addRow(cols.map { if (it == OpenableColumns.SIZE) f.length() else if (it == OpenableColumns.DISPLAY_NAME) name else null })
        }
    }

    override fun insert(uri: Uri, values: ContentValues?): Uri? = throw UnsupportedOperationException()
    override fun delete(uri: Uri, selection: String?, args: Array<out String>?) = 0
    override fun update(uri: Uri, values: ContentValues?, selection: String?, args: Array<out String>?) = 0
}
