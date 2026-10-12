package org.linktolinux.app

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.app.Activity
import androidx.core.content.ContextCompat

/** Returns asynchronous SmsManager failures to the paired Linux conversation. */
class SmsSentReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val requestId = intent.getStringExtra("request_id") ?: return
        val prefs = context.getSharedPreferences("message_sends", Context.MODE_PRIVATE)
        val remaining = (prefs.getInt("$requestId.remaining", 1) - 1).coerceAtLeast(0)
        val wasFailed = prefs.getBoolean("$requestId.failed", false)
        val failed = wasFailed || resultCode != Activity.RESULT_OK
        if (remaining == 0) {
            prefs.edit().remove("$requestId.remaining").remove("$requestId.failed").commit()
            java.io.File(context.cacheDir, "outgoing-mms/${requestId.filter { it.isLetterOrDigit() }}.pdu").delete()
        } else prefs.edit().putInt("$requestId.remaining", remaining).putBoolean("$requestId.failed", failed).commit()
        if (resultCode == Activity.RESULT_OK && (remaining > 0 || failed)) return
        if (wasFailed) return
        val serviceIntent = Intent(context, LinkService::class.java)
            .setAction(LinkService.ACTION_SMS_SENT)
            .putExtra("peer_id", intent.getStringExtra("peer_id"))
            .putExtra("sms_address", intent.getStringExtra("sms_address"))
            .putExtra("request_id", intent.getStringExtra("request_id"))
            .putExtra("result_code", resultCode)
            .putExtra("is_mms", intent.getBooleanExtra("is_mms", false))
        ContextCompat.startForegroundService(context, serviceIntent)
    }
}
