package org.survivalpedia.survivalpedia

import android.app.Activity
import android.content.ContentResolver
import android.content.Intent
import android.net.Uri
import android.provider.DocumentsContract
import android.provider.DocumentsContract.Document
import android.provider.OpenableColumns
import io.flutter.plugin.common.MethodChannel
import java.io.File
import java.io.FileOutputStream
import java.security.MessageDigest

/**
 * Importing the user's own EPUBs (Books tab, lib/book_import.dart): the system picker grants access to
 * a folder (searched with its subfolders) or to picked files, and every .epub is copied into the app as
 * `<dest>/<key>.epub`. When a book's folder holds its cover image next to it, the image is copied as
 * `<key>.side.jpg|png`. Dart then reads titles, authors and covers and puts the books on the shelf.
 */
object BookImport {
    private const val REQ_FOLDER = 41
    private const val REQ_FILES = 42
    private var pending: MethodChannel.Result? = null
    private var dest: File? = null

    private class Book(val uri: Uri, val name: String, val size: Long, val cover: Pair<Uri, String>?)

    fun start(a: Activity, folder: Boolean, destPath: String, result: MethodChannel.Result) {
        if (pending != null) {
            result.error("busy", "An import is already running", null)
            return
        }
        pending = result
        dest = File(destPath)
        val i = if (folder) {
            Intent(Intent.ACTION_OPEN_DOCUMENT_TREE)
        } else {
            Intent(Intent.ACTION_OPEN_DOCUMENT)
                .addCategory(Intent.CATEGORY_OPENABLE)
                .setType("*/*")
                .putExtra(Intent.EXTRA_MIME_TYPES, arrayOf("application/epub+zip", "application/octet-stream", "application/zip"))
                .putExtra(Intent.EXTRA_ALLOW_MULTIPLE, true)
        }
        try {
            a.startActivityForResult(i, if (folder) REQ_FOLDER else REQ_FILES)
        } catch (e: Exception) {
            pending = null
            result.error("picker", "No file picker on this phone", null)
        }
    }

    /** From MainActivity.onActivityResult; false when the request isn't ours. */
    fun onResult(a: Activity, req: Int, code: Int, data: Intent?): Boolean {
        if (req != REQ_FOLDER && req != REQ_FILES) return false
        val r = pending ?: return true
        pending = null
        if (code != Activity.RESULT_OK || data == null) {
            r.success(null)
            return true
        }
        val d = dest!!
        val cr = a.contentResolver
        Thread {
            try {
                d.mkdirs()
                val books = mutableListOf<Book>()
                if (req == REQ_FOLDER) {
                    val tree = data.data!!
                    walk(cr, tree, DocumentsContract.getTreeDocumentId(tree), books, 0)
                } else {
                    val uris = mutableListOf<Uri>()
                    data.clipData?.let { c -> for (k in 0 until c.itemCount) uris.add(c.getItemAt(k).uri) }
                    if (uris.isEmpty()) data.data?.let { uris.add(it) }
                    for (u in uris) {
                        var name = u.lastPathSegment ?: "book.epub"
                        var size = -1L
                        cr.query(u, arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE), null, null, null)?.use { c ->
                            if (c.moveToFirst()) {
                                c.getString(0)?.let { name = it }
                                if (!c.isNull(1)) size = c.getLong(1)
                            }
                        }
                        if (name.lowercase().endsWith(".epub")) books.add(Book(u, name, size, null))
                    }
                }
                val out = ArrayList<String>()
                for (b in books) {
                    val key = sha1("${b.name}|${b.size}").take(16)
                    val f = File(d, "$key.epub")
                    if (!f.exists()) {
                        val tmp = File(d, "$key.epub.tmp")
                        copy(cr, b.uri, tmp)
                        tmp.renameTo(f)
                        b.cover?.let { (u, ext) -> runCatching { copy(cr, u, File(d, "$key.side.$ext")) } }
                    }
                    out.add(f.path)
                }
                a.runOnUiThread { r.success(out) }
            } catch (e: Exception) {
                a.runOnUiThread { r.error("failed", e.message ?: e.toString(), null) }
            }
        }.start()
        return true
    }

    private fun walk(cr: ContentResolver, tree: Uri, docId: String, out: MutableList<Book>, depth: Int) {
        val children = DocumentsContract.buildChildDocumentsUriUsingTree(tree, docId)
        val epubs = mutableListOf<Triple<String, String, Long>>() // id, name, size
        val images = mutableMapOf<String, String>() // name -> id
        val dirs = mutableListOf<String>()
        cr.query(children, arrayOf(Document.COLUMN_DOCUMENT_ID, Document.COLUMN_DISPLAY_NAME, Document.COLUMN_MIME_TYPE, Document.COLUMN_SIZE), null, null, null)?.use { c ->
            while (c.moveToNext()) {
                val id = c.getString(0) ?: continue
                val name = c.getString(1) ?: ""
                val mime = c.getString(2) ?: ""
                val lower = name.lowercase()
                when {
                    mime == Document.MIME_TYPE_DIR -> dirs.add(id)
                    lower.endsWith(".epub") -> epubs.add(Triple(id, name, if (c.isNull(3)) -1L else c.getLong(3)))
                    lower.endsWith(".jpg") || lower.endsWith(".jpeg") || lower.endsWith(".png") -> images[name] = id
                }
            }
        }
        for ((id, name, size) in epubs) {
            val cover = sideCover(name, images.keys.toList())?.let { img ->
                DocumentsContract.buildDocumentUriUsingTree(tree, images.getValue(img)) to (if (img.lowercase().endsWith(".png")) "png" else "jpg")
            }
            out.add(Book(DocumentsContract.buildDocumentUriUsingTree(tree, id), name, size, cover))
        }
        if (depth < 12) for (d in dirs) walk(cr, tree, d, out, depth + 1)
    }

    /** Same rule as sideCover in book_import.dart: an image named like part of the book's file name, else the only image. */
    private fun sideCover(epub: String, images: List<String>): String? {
        val stem = epub.lowercase().removeSuffix(".epub")
        val named = images.filter {
            val s = it.lowercase().replace(Regex("\\.(jpe?g|png)$"), "")
            s.isNotEmpty() && stem.contains(s)
        }.sortedByDescending { it.length }
        return named.firstOrNull() ?: images.singleOrNull()
    }

    private fun copy(cr: ContentResolver, from: Uri, to: File) {
        val input = cr.openInputStream(from) ?: throw java.io.IOException("cannot open $from")
        input.use { i -> FileOutputStream(to).use { o -> i.copyTo(o, 256 * 1024) } }
    }

    private fun sha1(s: String): String =
        MessageDigest.getInstance("SHA-1").digest(s.toByteArray()).joinToString("") { "%02x".format(it) }
}
