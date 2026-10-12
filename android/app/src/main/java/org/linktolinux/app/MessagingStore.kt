package org.linktolinux.app

import android.Manifest
import android.content.Context
import android.content.ContentUris
import android.content.pm.PackageManager
import android.database.Cursor
import android.net.Uri
import android.provider.ContactsContract
import android.provider.Telephony
import org.json.JSONArray
import org.json.JSONObject

/** Reads Android's shared SMS/MMS store, preserving Android thread IDs and recipients. */
class MessagingStore(private val context: Context) {
    private val resolver = context.contentResolver
    private val names = mutableMapOf<String, String>()
    private val threadsUri = Telephony.Threads.CONTENT_URI.buildUpon().appendQueryParameter("simple", "true").build()
    private fun Cursor.string(column: String) = getString(getColumnIndexOrThrow(column)) ?: ""
    private fun Cursor.number(column: String) = getLong(getColumnIndexOrThrow(column))
    private fun clipped(value: String, bytes: Int): String {
        val encoded = value.toByteArray(Charsets.UTF_8)
        if (encoded.size <= bytes) return value
        var end = bytes
        while (end > 0 && (encoded[end].toInt() and 0xc0) == 0x80) end--
        return String(encoded, 0, end, Charsets.UTF_8)
    }
    fun name(address: String): String = names.getOrPut(address) {
        if (context.checkSelfPermission(Manifest.permission.READ_CONTACTS) != PackageManager.PERMISSION_GRANTED) address
        else resolver.query(Uri.withAppendedPath(ContactsContract.PhoneLookup.CONTENT_FILTER_URI, Uri.encode(address)),
            arrayOf(ContactsContract.PhoneLookup.DISPLAY_NAME), null, null, null)?.use {
            if (it.moveToFirst()) it.getString(0) ?: address else address
        } ?: address
    }
    fun recipients(thread: Long): List<String> {
        val ids = resolver.query(threadsUri, arrayOf("recipient_ids"), "_id=?", arrayOf(thread.toString()), null)?.use {
            if (it.moveToFirst()) it.getString(0) else null
        } ?: return emptyList()
        return ids.split(' ').mapNotNull { id ->
            id.toLongOrNull()?.let {
                resolver.query(ContentUris.withAppendedId(Uri.parse("content://mms-sms/canonical-address"), it),
                    null, null, null, null)?.use { c -> if (c.moveToFirst()) c.getString(0) else null }
            }
        }.filter { it.isNotBlank() && it != "insert-address-token" }.distinct()
    }
    fun contacts(query: String, offset: Int): JSONObject {
        check(context.checkSelfPermission(Manifest.permission.READ_CONTACTS) == PackageManager.PERMISSION_GRANTED) {
            "Allow Contacts access in the Android app to choose contacts"
        }
        val uri = if (query.isBlank()) ContactsContract.CommonDataKinds.Phone.CONTENT_URI else
            Uri.withAppendedPath(ContactsContract.CommonDataKinds.Phone.CONTENT_FILTER_URI, Uri.encode(query))
        val rows = JSONArray()
        var more = false
        resolver.query(uri, arrayOf("_id", "display_name", "data1"), null, null, "display_name COLLATE NOCASE ASC, _id ASC")?.use { c ->
            if (c.moveToPosition(offset)) do {
                if (rows.length() == 100) { more = true; break }
                rows.put(JSONObject().put("id", c.string("_id")).put("name", clipped(c.string("display_name"), 256))
                    .put("address", clipped(c.string("data1"), 128)))
            } while (c.moveToNext())
        }
        return JSONObject().put("contacts", rows).put("offset", offset).put("query", query)
            .put("next_offset", if (more) offset + 100 else JSONObject.NULL)
    }
    fun threads(offset: Int): JSONObject {
        val rows = JSONArray()
        var more = false
        resolver.query(threadsUri, arrayOf("_id", "date", "snippet"), "message_count>0", null, "date DESC, _id DESC")?.use { c ->
            if (c.moveToPosition(offset)) do {
                if (rows.length() == 100) { more = true; break }
                val id = c.number("_id")
                val recipients = recipients(id)
                rows.put(JSONObject().put("id", id.toString()).put("address", recipients.joinToString(";"))
                    .put("display_name", clipped(recipients.joinToString(", ") { name(it) }, 512))
                    .put("snippet", clipped(c.string("snippet").ifBlank { "Multimedia message" }, 512)).put("date", c.number("date")))
            } while (c.moveToNext())
        }
        return JSONObject().put("threads", rows).put("offset", offset).put("next_offset", if (more) offset + 100 else JSONObject.NULL)
    }
    private fun mmsText(id: Long): String {
        val parts = mutableListOf<String>()
        resolver.query(Uri.parse("content://mms/$id/part"), arrayOf("_id", "ct", "text", "_data"), null, null, "seq ASC")?.use { c ->
            while (c.moveToNext()) {
                when (val type = c.string("ct")) {
                    "text/plain", "text/html" -> {
                        val text = if (c.string("_data").isBlank()) c.string("text") else
                            resolver.openInputStream(Uri.parse("content://mms/part/${c.number("_id")}"))?.use {
                                val buffer = ByteArray(32768)
                                var count = 0
                                while (count < buffer.size) {
                                    val read = it.read(buffer, count, buffer.size - count)
                                    if (read <= 0) break
                                    count += read
                                }
                                String(buffer, 0, count, Charsets.UTF_8)
                            } ?: ""
                        if (text.isNotBlank()) parts.add(text)
                    }
                    "application/smil" -> Unit
                    else -> parts.add(when {
                        type.startsWith("image/") -> "[Photo]"
                        type.startsWith("video/") -> "[Video]"
                        type.startsWith("audio/") -> "[Audio]"
                        else -> "[Attachment]"
                    })
                }
            }
        }
        return clipped(parts.joinToString("\n").ifBlank { "[MMS awaiting download on phone]" }, 32768)
    }
    private fun mmsSender(id: Long): String = resolver.query(Uri.parse("content://mms/$id/addr"), arrayOf("address"),
        "type=137", null, null)?.use { if (it.moveToFirst()) it.getString(0) else "" } ?: ""

    fun history(thread: Long, offset: Int): JSONObject {
        val rows = mutableListOf<JSONObject>()
        val recipients = recipients(thread)
        // Merge two sorted cursors. SMS timestamps are milliseconds; MMS uses seconds.
        resolver.query(Telephony.Sms.CONTENT_URI, arrayOf("_id", "address", "body", "date", "type"),
            "thread_id=? AND type!=3", arrayOf(thread.toString()), "date DESC, _id DESC")?.use { sms ->
            resolver.query(Telephony.Mms.CONTENT_URI, arrayOf("_id", "date", "msg_box"),
                "thread_id=? AND msg_box!=3", arrayOf(thread.toString()), "date DESC, _id DESC")?.use { mms ->
                var hasSms = sms.moveToFirst()
                var hasMms = mms.moveToFirst()
                var index = 0
                while ((hasSms || hasMms) && rows.size < 101) {
                    val useSms = hasSms && (!hasMms || sms.number("date") >= mms.number("date") * 1000)
                    val c = if (useSms) sms else mms
                    if (index++ >= offset) {
                        val id = c.number("_id")
                        val kind = c.number(if (useSms) "type" else "msg_box").toInt()
                        val incoming = kind == 1
                        val sender = if (useSms) c.string("address") else if (incoming) mmsSender(id) else ""
                        val state = if (incoming) "Received" else if (kind == 2) "Sent" else if (useSms && kind == 5) "Failed" else "Pending"
                        rows.add(JSONObject().put("id", "${if (useSms) "sms" else "mms"}:$id")
                            .put("address", clipped(sender, 128)).put("sender_name", clipped(if (incoming) name(sender) else "You", 256))
                            .put("body", if (useSms) clipped(c.string("body"), 32768) else mmsText(id))
                            .put("date", c.number("date") * if (useSms) 1 else 1000)
                            .put("direction", if (incoming) "inbound" else "outbound").put("state", state))
                    }
                    if (useSms) hasSms = sms.moveToNext() else hasMms = mms.moveToNext()
                }
            }
        }
        val more = rows.size > 100
        return JSONObject().put("thread_id", thread.toString()).put("address", recipients.joinToString(";"))
            .put("display_name", clipped(recipients.joinToString(", ") { name(it) }, 128))
            .put("messages", JSONArray(rows.take(100).reversed())).put("offset", offset)
            .put("next_offset", if (more) offset + 100 else JSONObject.NULL)
    }
}
