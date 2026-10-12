package org.linktolinux.app

import android.service.notification.NotificationListenerService
import android.service.notification.StatusBarNotification
import android.os.Build
import java.util.concurrent.Executors

/**
 * Journals Android notifications and forwards them to paired Linux devices
 * only after the user enables forwarding in the app.
 */
class LinkNotifications : NotificationListenerService() {

    companion object {
        init {
            System.loadLibrary("link_android_svc")
        }
    }

    external fun nativeOnNotification(filesDir: String, id: String, title: String, body: String): Int
    external fun nativeSendNotification(peerId: String, address: String, id: String, appName: String, title: String, body: String): String
    private val worker = Executors.newSingleThreadExecutor()

    override fun onNotificationPosted(sbn: StatusBarNotification) {
        val persistentFlags = android.app.Notification.FLAG_ONGOING_EVENT or
            android.app.Notification.FLAG_NO_CLEAR or
            android.app.Notification.FLAG_FOREGROUND_SERVICE
        if (sbn.packageName == packageName || sbn.isOngoing ||
            (sbn.notification.flags and persistentFlags) != 0) return

        val prefs = getSharedPreferences("link", MODE_PRIVATE)
        val appName = try {
            packageManager.getApplicationLabel(packageManager.getApplicationInfo(sbn.packageName, 0)).toString()
        } catch (_: Exception) { sbn.packageName }
        rememberApp(sbn.packageName, appName)
        if (!prefs.getBoolean("notification_forwarding", false) ||
            prefs.getStringSet("notification_blocked_apps", emptySet())?.contains(sbn.packageName) == true) return

        val extras = sbn.notification.extras
        val title = limitUtf8(extras.getCharSequence("android.title")?.toString().orEmpty(), 512)
        val text = limitUtf8(extras.getCharSequence("android.text")?.toString().orEmpty(), 4096)
        if (title.isBlank() && text.isBlank()) return
        nativeOnNotification(filesDir.absolutePath, sbn.key, title, text)
        worker.execute {
            val emulator = Build.FINGERPRINT.startsWith("generic") || Build.MODEL.contains("sdk_gphone")
            TrustedPeers.load(filesDir).forEach { peer ->
                val address = if (emulator && peer.address.startsWith("127.0.0.1:"))
                    "10.0.2.2:${peer.address.substringAfterLast(':')}" else peer.address
                val response = nativeSendNotification(peer.id, address, sbn.key, limitUtf8(appName, 256), title, text)
                if (!response.contains("\"ok\":true")) android.util.Log.w("LinkToLinux", "Notification forwarding failed: $response")
            }
        }
    }

    private fun rememberApp(packageName: String, appName: String) {
        synchronized(LinkNotifications::class.java) {
            val prefs = getSharedPreferences("link", MODE_PRIVATE)
            val labels = try {
                org.json.JSONObject(prefs.getString("notification_seen_apps", "{}") ?: "{}")
            } catch (_: Exception) { org.json.JSONObject() }
            labels.put(packageName, appName)
            prefs.edit().putString("notification_seen_apps", labels.toString()).apply()
        }
    }

    private fun limitUtf8(value: String, maxBytes: Int): String {
        val out = StringBuilder()
        var bytes = 0
        value.codePoints().forEach { codePoint ->
            val piece = String(Character.toChars(codePoint))
            val size = piece.toByteArray(Charsets.UTF_8).size
            if (bytes + size <= maxBytes) { out.append(piece); bytes += size }
        }
        return out.toString()
    }

    override fun onDestroy() { worker.shutdown(); super.onDestroy() }
}
