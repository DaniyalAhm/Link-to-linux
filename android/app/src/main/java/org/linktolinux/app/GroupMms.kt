package org.linktolinux.app

import android.app.PendingIntent
import android.content.Context
import android.os.Bundle
import android.telephony.SmsManager
import androidx.core.content.FileProvider
import com.google.android.mms.pdu_alt.EncodedStringValue
import com.google.android.mms.pdu_alt.PduBody
import com.google.android.mms.pdu_alt.PduComposer
import com.google.android.mms.pdu_alt.PduPart
import com.google.android.mms.pdu_alt.SendReq
import java.io.File

/** One MMS with all recipients, so replies remain in a real carrier group thread. */
object GroupMms {
    fun compose(context: Context, recipients: List<String>, text: String): ByteArray {
        require(recipients.size in 2..20) { "A group must have 2–20 recipients" }
        val part = PduPart().apply {
            charset = 106 // UTF-8
            contentType = "text/plain".toByteArray()
            contentId = "text".toByteArray()
            contentLocation = "message.txt".toByteArray()
            data = text.toByteArray(Charsets.UTF_8)
        }
        val request = SendReq().apply {
            setTo(recipients.map { EncodedStringValue(it) }.toTypedArray())
            date = System.currentTimeMillis() / 1000
            contentType = "application/vnd.wap.multipart.mixed".toByteArray()
            body = PduBody().apply { addPart(part) }
            messageSize = part.data.size.toLong()
        }
        return PduComposer(context, request).make() ?: error("Could not encode group MMS")
    }
    fun send(context: Context, sms: SmsManager, recipients: List<String>, text: String, callback: PendingIntent, requestId: String) {
        val dir = File(context.cacheDir, "outgoing-mms").apply { mkdirs() }
        dir.listFiles()?.filter { System.currentTimeMillis() - it.lastModified() > 86400000 }?.forEach { it.delete() }
        val file = File(dir, "${requestId.filter { it.isLetterOrDigit() }}.pdu")
        file.writeBytes(compose(context, recipients, text))
        val uri = FileProvider.getUriForFile(context, "${context.packageName}.outgoing-mms", file)
        val options = Bundle().apply { putBoolean(SmsManager.MMS_CONFIG_GROUP_MMS_ENABLED, true) }
        sms.sendMultimediaMessage(context, uri, null, options, callback)
    }
}
