package org.linktolinux.app

import android.content.ContentProvider
import android.content.ContentValues
import android.database.Cursor
import android.database.MatrixCursor
import android.net.Uri
import android.os.ParcelFileDescriptor
import android.provider.OpenableColumns
import java.io.FileNotFoundException
import java.io.FileOutputStream

/** Exposes the latest received image as an in-memory content URI for Android clipboard consumers. */
class ClipboardImageProvider : ContentProvider() {
    companion object {
        private data class PublishedImage(val mime: String, val bytes: ByteArray)

        // Publish MIME and bytes together so a clipboard reader can never see
        // metadata from one image with the payload from another.
        @Volatile private var published = PublishedImage("image/png", ByteArray(0))

        fun publish(context: android.content.Context, contentType: String, bytes: ByteArray): Uri {
            published = PublishedImage(contentType, bytes.copyOf())
            return Uri.parse("content://${context.packageName}.clipboard/image/latest")
        }
    }

    override fun onCreate() = true
    override fun getType(uri: Uri): String = published.mime
    override fun query(uri: Uri, projection: Array<out String>?, selection: String?, selectionArgs: Array<out String>?, sortOrder: String?): Cursor {
        val columns = projection ?: arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE)
        return MatrixCursor(columns).apply {
            addRow(columns.map { column -> when (column) {
                OpenableColumns.DISPLAY_NAME -> "clipboard-image"
                OpenableColumns.SIZE -> published.bytes.size
                else -> null
            } })
        }
    }
    override fun openFile(uri: Uri, mode: String): ParcelFileDescriptor {
        if (mode != "r" || uri.lastPathSegment != "latest") throw FileNotFoundException("clipboard image unavailable")
        val pipe = ParcelFileDescriptor.createPipe()
        val snapshot = published.bytes
        Thread({
            try { FileOutputStream(pipe[1].fileDescriptor).use { it.write(snapshot) } }
            catch (_: Exception) { }
            finally { try { pipe[1].close() } catch (_: Exception) { } }
        }, "clipboard-image-reader").start()
        return pipe[0]
    }
    override fun insert(uri: Uri, values: ContentValues?) = null
    override fun delete(uri: Uri, selection: String?, selectionArgs: Array<out String>?) = 0
    override fun update(uri: Uri, values: ContentValues?, selection: String?, selectionArgs: Array<out String>?) = 0
}
