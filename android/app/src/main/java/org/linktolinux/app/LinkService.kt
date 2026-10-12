package org.linktolinux.app

import android.app.Notification
import android.app.Activity
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.ClipData
import android.content.ContentResolver
import android.content.ClipDescription
import android.content.ClipboardManager
import android.content.Intent
import android.os.Build
import android.os.IBinder
import android.os.Handler
import android.os.Looper
import android.os.Bundle
import android.util.Base64
import android.provider.MediaStore
import android.provider.ContactsContract
import android.provider.Telephony
import android.telephony.SmsManager
import android.content.ContentUris
import android.content.pm.PackageManager
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import org.json.JSONObject
import java.util.concurrent.Executors
import java.util.UUID
import android.util.Log
import androidx.core.app.NotificationCompat

/** Foreground service: notification + Rust TLS server (Phase 1b). */
class LinkService : Service() {

    companion object {
        init {
            System.loadLibrary("link_android_svc")
        }
        private val worker = Executors.newSingleThreadExecutor()
        const val CHANNEL = "link-service"
        const val TLS_PORT = 53536
        const val ACTION_SEND_CLIPBOARD = "org.linktolinux.app.SEND_CLIPBOARD"
        const val EXTRA_NOTIFICATION_TOKEN = "notification_token"
        const val PREF_NOTIFICATION_TOKEN = "notification_send_token"
        const val ACTION_SMS_SENT = "org.linktolinux.app.SMS_SENT"
    }

    private val main = Handler(Looper.getMainLooper())
    private lateinit var lan: LanDiscovery
    private var destroyed = false
    external fun nativeLocalIdentityJson(deviceName: String, filesDir: String): String

    external fun nativeStartTlsServer(filesDir: String, port: Int, deviceName: String): String
    external fun nativeStopTlsServer(): Int
    external fun nativeClipboardEnabled(enabled: Boolean): Int
    external fun nativeClipboardStatus(): String
    external fun nativeClipboardTake(): String
    external fun nativeGalleryTake(): String
    external fun nativeGallerySend(id: String, address: String, packet: String): String
    external fun nativeMessagesTake(): String
    external fun nativeMessagesSend(id: String, address: String, packet: String): String

    private var clipboardPollInFlight = false
    private var localDeviceId = ""
    private val clipboardPoll = object : Runnable {
        override fun run() {
            if (destroyed) return
            if (clipboardPollInFlight) {
                main.postDelayed(this, 250)
                return
            }
            clipboardPollInFlight = true
            worker.execute {
                val pending = try {
                    if (JSONObject(nativeClipboardStatus()).optBoolean("enabled")) {
                        JSONObject(nativeClipboardTake()).optJSONArray("clipboard")
                    } else null
                } catch (e: Exception) {
                    Log.w("LinkToLinux", "background clipboard poll failed: ${e.message}")
                    null
                }
                try {
                    val gallery = JSONObject(nativeGalleryTake()).optJSONObject("gallery")
                    if (gallery != null) handleGalleryRequest(gallery)
                } catch (e: Exception) { Log.w("LinkToLinux", "gallery request failed: ${e.message}") }
                try {
                    val messages = JSONObject(nativeMessagesTake()).optJSONObject("messages")
                    if (messages != null) handleMessagesRequest(messages)
                } catch (e: Exception) { Log.w("LinkToLinux", "messages request failed: ${e.message}") }
                main.post {
                    clipboardPollInFlight = false
                    if (!destroyed) {
                        if (pending != null && pending.length() >= 3) {
                            applyRemoteClipboard(pending.optString(1), pending.optString(2))
                        }
                        main.postDelayed(this, 750)
                    }
                }
            }
        }
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        lan = LanDiscovery(this)
        ensureChannel()
        val notificationToken = UUID.randomUUID().toString()
        getSharedPreferences("link", MODE_PRIVATE).edit()
            .putString(PREF_NOTIFICATION_TOKEN, notificationToken)
            .apply()
        val sendClipboardIntent = Intent(this, MainActivity::class.java)
            .setAction(ACTION_SEND_CLIPBOARD)
            .putExtra(EXTRA_NOTIFICATION_TOKEN, notificationToken)
            .addFlags(Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP)
        val sendClipboardPendingIntent = PendingIntent.getActivity(
            this,
            1,
            sendClipboardIntent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        val notif: Notification = NotificationCompat.Builder(this, CHANNEL)
            .setContentTitle(getString(R.string.app_name))
            .setContentText("Link active · tap Send clipboard to share a phone clip")
            .setSmallIcon(android.R.drawable.stat_sys_data_bluetooth)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .addAction(android.R.drawable.ic_menu_send, "Send clipboard", sendClipboardPendingIntent)
            .build()
        startForeground(1, notif)
        main.postDelayed(clipboardPoll, 750)

        // Rust owns the server socket on its own thread; never block here.
        val dir = filesDir.absolutePath
        val name = phoneDisplayName()
        worker.execute {
            val res = nativeStartTlsServer(dir, TLS_PORT, name)
            Log.i("LinkToLinux", "tls server start -> $res")
            try {
                if (JSONObject(res).optBoolean("ok")) {
                    val clipboardEnabled = getSharedPreferences("link", MODE_PRIVATE)
                        .getBoolean("clipboard_sync", false)
                    nativeClipboardEnabled(clipboardEnabled)
                    val identity = JSONObject(nativeLocalIdentityJson(name, dir))
                    localDeviceId = identity.optString("id")
                    main.post {
                        if (!destroyed) lan.advertise(identity) { Log.e("LinkToLinux", it) }
                    }
                }
            } catch (e: Exception) { Log.e("LinkToLinux", "LAN advertisement failed", e) }
        }
    }

    override fun onDestroy() {
        destroyed = true
        main.removeCallbacks(clipboardPoll)
        lan.close()
        worker.execute { nativeStopTlsServer() }
        super.onDestroy()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == ACTION_SMS_SENT) {
            val peerId = intent.getStringExtra("peer_id") ?: return START_STICKY
            val address = intent.getStringExtra("sms_address") ?: return START_STICKY
            val requestId = intent.getStringExtra("request_id") ?: return START_STICKY
            val resultCode = intent.getIntExtra("result_code", Activity.RESULT_CANCELED)
            run {
                worker.execute {
                    val reason = if (intent.getBooleanExtra("is_mms", false)) "MMS could not be sent (carrier error $resultCode). Check mobile data and MMS settings on your phone." else when (resultCode) {
                        SmsManager.RESULT_ERROR_RADIO_OFF -> "The phone radio is off; SMS was not sent"
                        SmsManager.RESULT_ERROR_NO_SERVICE -> "No mobile service; SMS was not sent"
                        SmsManager.RESULT_ERROR_LIMIT_EXCEEDED -> "SMS send limit reached"
                        SmsManager.RESULT_ERROR_FDN_CHECK_FAILURE -> "The carrier blocked this number (fixed dialing restriction)"
                        else -> "Android could not send this SMS (error $resultCode)"
                    }
                    val peer = TrustedPeers.load(filesDir).firstOrNull { it.id == peerId } ?: return@execute
                    sendMessages(peer.id, peer.address, JSONObject()
                        .put("op", if (resultCode == Activity.RESULT_OK) "messages_sent" else "messages_error").put("from", localDeviceId)
                        .put("request_id", requestId).put("address", address)
                        .put("error", if (resultCode == Activity.RESULT_OK) JSONObject.NULL else reason))
                }
            }
        }
        return START_STICKY
    }

    private fun ensureChannel() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val mgr = getSystemService(NotificationManager::class.java)
            mgr.createNotificationChannel(
                NotificationChannel(CHANNEL, "Link", NotificationManager.IMPORTANCE_LOW)
            )
        }
    }

    private fun applyRemoteClipboard(mime: String, encoded: String) {
        if (mime !in listOf("text/plain", "image/png", "image/jpeg", "image/webp")) return
        val bytes = try { Base64.decode(encoded, Base64.NO_WRAP) } catch (_: IllegalArgumentException) { return }
        if ((mime == "text/plain" && (bytes.isEmpty() || bytes.size > 32 * 1024)) ||
            (mime.startsWith("image/") && bytes.size > 5 * 1024 * 1024)) return
        try {
            val clipboard = getSystemService(CLIPBOARD_SERVICE) as ClipboardManager
            if (mime == "text/plain") {
                clipboard.setPrimaryClip(ClipData.newPlainText("Link to Linux", bytes.toString(Charsets.UTF_8)))
            } else {
                val uri = ClipboardImageProvider.publish(this, mime, bytes)
                // newUri asks the provider for its image MIME type and places that
                // type on ClipDescription for paste targets to inspect.
                clipboard.setPrimaryClip(ClipData.newUri(contentResolver, "Link to Linux image", uri))
            }
            Log.i("LinkToLinux", "applied background clipboard update · $mime")
        } catch (e: Exception) {
            Log.w("LinkToLinux", "could not apply background clipboard update: ${e.message}")
        }
    }

    private fun hasGalleryPermission(): Boolean = when {
        Build.VERSION.SDK_INT >= 34 -> checkSelfPermission(android.Manifest.permission.READ_MEDIA_IMAGES) == PackageManager.PERMISSION_GRANTED || checkSelfPermission("android.permission.READ_MEDIA_VISUAL_USER_SELECTED") == PackageManager.PERMISSION_GRANTED
        Build.VERSION.SDK_INT >= 33 -> checkSelfPermission(android.Manifest.permission.READ_MEDIA_IMAGES) == PackageManager.PERMISSION_GRANTED
        else -> checkSelfPermission(android.Manifest.permission.READ_EXTERNAL_STORAGE) == PackageManager.PERMISSION_GRANTED
    }

    private fun handleGalleryRequest(request: JSONObject) {
        val op=request.optString("op")
        val requestId=request.optString("request_id")
        val peerId=request.optString("from")
        val peer=TrustedPeers.load(filesDir).firstOrNull { it.id == peerId } ?: return
        try {
            if (!hasGalleryPermission()) throw SecurityException("Allow gallery access in the Android app to browse photos")
            when (op) {
                "gallery_list" -> {
                    val offset=request.optInt("offset",0).coerceIn(0,100000)
                    val items=org.json.JSONArray()
                    val projection=arrayOf(MediaStore.Images.Media._ID,MediaStore.Images.Media.DISPLAY_NAME,MediaStore.Images.Media.MIME_TYPE,MediaStore.Images.Media.DATE_MODIFIED,MediaStore.Images.Media.SIZE)
                    val args=Bundle().apply { putString(ContentResolver.QUERY_ARG_SQL_SORT_ORDER,"${MediaStore.Images.Media.DATE_MODIFIED} DESC"); putInt(ContentResolver.QUERY_ARG_LIMIT,40); putInt(ContentResolver.QUERY_ARG_OFFSET,offset) }
                    contentResolver.query(MediaStore.Images.Media.EXTERNAL_CONTENT_URI,projection,args,null)?.use { cursor ->
                        val idCol=cursor.getColumnIndexOrThrow(MediaStore.Images.Media._ID); val nameCol=cursor.getColumnIndexOrThrow(MediaStore.Images.Media.DISPLAY_NAME); val mimeCol=cursor.getColumnIndexOrThrow(MediaStore.Images.Media.MIME_TYPE); val dateCol=cursor.getColumnIndexOrThrow(MediaStore.Images.Media.DATE_MODIFIED); val sizeCol=cursor.getColumnIndexOrThrow(MediaStore.Images.Media.SIZE)
                        while(cursor.moveToNext()) {
                            val photoId=cursor.getLong(idCol)
                            val item=JSONObject().put("id",photoId.toString()).put("name",cursor.getString(nameCol) ?: "Photo").put("mime",cursor.getString(mimeCol) ?: "image/jpeg").put("date_modified",cursor.getLong(dateCol)).put("size",cursor.getLong(sizeCol).coerceAtLeast(0))
                            galleryThumbnail(photoId)?.let { (mime, encoded) -> item.put("thumbnail_mime",mime).put("thumbnail_b64",encoded) }
                            items.put(item)
                        }
                    }
                    val response=JSONObject().put("op","gallery_list_result").put("from",localDeviceId).put("request_id",requestId).put("offset",offset).put("items",items).put("next_offset",if(items.length()==40) offset+40 else JSONObject.NULL)
                    sendGallery(peer.id,peer.address,response)
                }
                "gallery_fetch" -> {
                    val photoId=request.optString("photo_id").toLongOrNull() ?: throw IllegalArgumentException("Invalid photo ID")
                    val uri=ContentUris.withAppendedId(MediaStore.Images.Media.EXTERNAL_CONTENT_URI,photoId)
                    val raw=contentResolver.openInputStream(uri)?.use { input -> val out=java.io.ByteArrayOutputStream(); val buffer=ByteArray(16*1024); var total=0; while(true){val n=input.read(buffer);if(n<0)break;total+=n;require(total<=40*1024*1024){"Photo exceeds 40 MiB"};out.write(buffer,0,n)};out.toByteArray() } ?: throw java.io.IOException("Photo is unavailable")
                    val (mime,bytes)=prepareGalleryImage(raw)
                    val name=contentResolver.query(uri,arrayOf(MediaStore.Images.Media.DISPLAY_NAME),null,null,null)?.use { if(it.moveToFirst()) it.getString(0) else null } ?: "photo-$photoId"
                    val response=JSONObject().put("op","gallery_image").put("from",localDeviceId).put("request_id",requestId).put("photo_id",photoId.toString()).put("name",name).put("mime",mime).put("data_b64",Base64.encodeToString(bytes,Base64.NO_WRAP))
                    sendGallery(peer.id,peer.address,response)
                }
            }
        } catch(e:Exception) { sendGallery(peer.id,peer.address,JSONObject().put("op","gallery_error").put("from",localDeviceId).put("request_id",requestId).put("error",e.message ?: "Gallery request failed")) }
    }

    private fun sendGallery(peerId:String,address:String,packet:JSONObject) {
        val result=JSONObject(nativeGallerySend(peerId,address,packet.toString()))
        if(!result.optBoolean("ok")) Log.w("LinkToLinux","gallery response rejected: ${result.optString("error")}")
    }

    private fun hasSmsPermissions(): Boolean =
        checkSelfPermission(android.Manifest.permission.READ_SMS) == PackageManager.PERMISSION_GRANTED &&
            checkSelfPermission(android.Manifest.permission.SEND_SMS) == PackageManager.PERMISSION_GRANTED

    private fun contactDisplayName(address: String): String? {
        if (checkSelfPermission(android.Manifest.permission.READ_CONTACTS) != PackageManager.PERMISSION_GRANTED) return null
        return try {
            val lookup = android.net.Uri.withAppendedPath(
                ContactsContract.PhoneLookup.CONTENT_FILTER_URI,
                android.net.Uri.encode(address),
            )
            contentResolver.query(lookup, arrayOf(ContactsContract.PhoneLookup.DISPLAY_NAME), null, null, null)?.use { cursor ->
                if (cursor.moveToFirst()) cursor.getString(0)?.takeIf { it.isNotBlank() } else null
            }
        } catch (e: Exception) {
            Log.w("LinkToLinux", "contact lookup failed: ${e.message}")
            null
        }
    }

    private fun handleMessagesRequest(request: JSONObject) {
        val op = request.optString("op")
        val requestId = request.optString("request_id")
        val peerId = request.optString("from")
        val peer = TrustedPeers.load(filesDir).firstOrNull { it.id == peerId } ?: return
        Log.i("LinkToLinux", "received SMS operation $op from paired Linux peer")
        try {
            if (op == "messages_status") {
                val enabled = getSharedPreferences("link", MODE_PRIVATE)
                    .getBoolean("messages_enabled", false) && hasSmsPermissions()
                sendMessages(peer.id, peer.address, JSONObject()
                    .put("op", "messages_status_result").put("from", localDeviceId)
                    .put("request_id", requestId).put("enabled", enabled)
                    .put("device_name", phoneDisplayName()))
                return
            }
            if (!getSharedPreferences("link", MODE_PRIVATE).getBoolean("messages_enabled", false)) {
                throw SecurityException("Enable SMS sharing in the Android app to use messages from Linux")
            }
            if (!hasSmsPermissions()) throw SecurityException("Enable SMS sharing and grant read and send access in the Android app")
            when (op) {
                "messages_list", "messages_thread", "messages_contacts" -> {
                    val offset = request.optInt("offset", 0)
                    require(offset in 0..100000) { "Invalid page" }
                    val store = MessagingStore(this)
                    val result = when (op) {
                        "messages_contacts" -> store.contacts(request.optString("query").take(128), offset)
                        "messages_thread" -> store.history(request.optString("thread_id").toLongOrNull()
                            ?: throw IllegalArgumentException("Invalid conversation"), offset)
                        else -> store.threads(offset)
                    }
                    result.put("op", "${op}_result").put("from", localDeviceId)
                        .put("request_id", requestId).put("device_name", phoneDisplayName())
                    sendMessages(peer.id, peer.address, result)
                }
                "messages_send" -> {
                    val address = request.optString("address").trim()
                    val body = request.optString("body")
                    require(address.isNotEmpty() && address.length <= 2048) { "Enter a valid phone number" }
                    require(body.isNotBlank() && body.toByteArray(Charsets.UTF_8).size <= 4096) { "Message is empty or too long" }
                    val recipients = address.split(';', ',').map { it.trim() }.filter { it.isNotEmpty() }.distinct()
                    require(recipients.size in 1..20 && recipients.all { it.length <= 128 && it.matches(Regex("[+0-9 ()\\-.]+")) }) { "Choose valid phone numbers (up to 20 recipients)" }
                    val threadId = Telephony.Threads.getOrCreateThreadId(this, recipients.toSet())
                    val sms = smsManager()
                    val parts = if (recipients.size > 1) arrayListOf(body) else sms.divideMessage(body)
                    require(parts.size <= 30) { "SMS is too long to send" }
                    val sentIntents = ArrayList(parts.mapIndexed { index, _ ->
                        val callback = Intent(this, SmsSentReceiver::class.java)
                            .setAction("${ACTION_SMS_SENT}.$requestId.$index")
                            .putExtra("peer_id", peer.id)
                            .putExtra("sms_address", address)
                            .putExtra("request_id", requestId)
                            .putExtra("is_mms", recipients.size > 1)
                        PendingIntent.getBroadcast(
                            this,
                            31000 + (requestId.hashCode() and 0x3fff) + index,
                            callback,
                            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
                        )
                    })
                    getSharedPreferences("message_sends", MODE_PRIVATE).edit()
                        .putInt("$requestId.remaining", parts.size).putBoolean("$requestId.failed", false).commit()
                    if (recipients.size > 1) GroupMms.send(this, sms, recipients, body, sentIntents.first(), requestId)
                    else if (parts.size <= 1) sms.sendTextMessage(recipients.first(), null, body, sentIntents.first(), null)
                    else sms.sendMultipartTextMessage(recipients.first(), null, ArrayList(parts), sentIntents, null)
                    Log.i("LinkToLinux", "submitted SMS to Android telephony")
                    sendMessages(peer.id, peer.address, JSONObject()
                        .put("op", "messages_send_result").put("from", localDeviceId)
                        .put("request_id", requestId).put("thread_id", threadId.toString())
                        .put("address", address).put("body", body))
                }
                else -> throw IllegalArgumentException("Unknown messages request")
            }
        } catch (e: Exception) {
            Log.w("LinkToLinux", "SMS operation $op failed: ${e.message}")
            sendMessages(peer.id, peer.address, JSONObject()
                .put("op", "messages_error").put("from", localDeviceId)
                .put("request_id", requestId).put("query", if (op == "messages_contacts") request.optString("query") else JSONObject.NULL)
                .put("error", e.message ?: "Message request failed"))
        }
    }

    private fun sendMessages(peerId: String, address: String, packet: JSONObject) {
        val result = JSONObject(nativeMessagesSend(peerId, address, packet.toString()))
        if (!result.optBoolean("ok")) Log.w("LinkToLinux", "messages response rejected: ${result.optString("error")}")
    }

    @Suppress("DEPRECATION")
    private fun smsManager(): SmsManager = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S)
        getSystemService(SmsManager::class.java) else SmsManager.getDefault()

    private fun limitUtf8Text(value: String, maxBytes: Int): String {
        val out = StringBuilder()
        var bytes = 0
        value.codePoints().forEach { point ->
            val character = String(Character.toChars(point))
            val length = character.toByteArray(Charsets.UTF_8).size
            if (bytes + length <= maxBytes) { out.append(character); bytes += length }
        }
        return out.toString()
    }

    private fun galleryThumbnail(photoId:Long):Pair<String,String>? {
        val bitmap=try { MediaStore.Images.Thumbnails.getThumbnail(contentResolver,photoId,MediaStore.Images.Thumbnails.MINI_KIND,null) } catch (_:Exception) { null } ?: return null
        try {
            for (quality in listOf(55,40,30)) {
                val output=java.io.ByteArrayOutputStream()
                if(bitmap.compress(Bitmap.CompressFormat.WEBP,quality,output)&&output.size()<=48*1024) return "image/webp" to Base64.encodeToString(output.toByteArray(),Base64.NO_WRAP)
            }
            val scaled=Bitmap.createScaledBitmap(bitmap,256,192,true)
            try { val output=java.io.ByteArrayOutputStream();if(scaled.compress(Bitmap.CompressFormat.WEBP,25,output)&&output.size()<=48*1024)return "image/webp" to Base64.encodeToString(output.toByteArray(),Base64.NO_WRAP) }
            finally { if(scaled!==bitmap)scaled.recycle() }
        } finally { bitmap.recycle() }
        return null
    }

    private fun prepareGalleryImage(raw:ByteArray):Pair<String,ByteArray> {
        val mime=when { raw.size>=8 && raw.copyOfRange(0,8).contentEquals(byteArrayOf(0x89.toByte(),0x50,0x4e,0x47,0x0d,0x0a,0x1a,0x0a))->"image/png"; raw.size>=3&&raw[0]==0xff.toByte()&&raw[1]==0xd8.toByte()->"image/jpeg"; raw.size>=12&&String(raw,0,4)=="RIFF"&&String(raw,8,4)=="WEBP"->"image/webp";else->null }
        if(raw.size<=5*1024*1024&&mime!=null)return mime to raw
        val bounds=BitmapFactory.Options().apply{inJustDecodeBounds=true};BitmapFactory.decodeByteArray(raw,0,raw.size,bounds)
        require(bounds.outWidth>0&&bounds.outHeight>0){"Unsupported image format"};var sample=1
        while((bounds.outWidth/sample).toLong()*(bounds.outHeight/sample)>4_000_000L)sample*=2
        while(sample<=64){val bitmap=BitmapFactory.decodeByteArray(raw,0,raw.size,BitmapFactory.Options().apply{inSampleSize=sample;inPreferredConfig=Bitmap.Config.ARGB_8888}) ?: break
            try { for(q in listOf(88,78,68,58)){val out=java.io.ByteArrayOutputStream();if(bitmap.compress(Bitmap.CompressFormat.WEBP,q,out)&&out.size()<=5*1024*1024)return "image/webp" to out.toByteArray()} } finally {bitmap.recycle()};sample*=2 }
        throw java.io.IOException("Photo could not be reduced below 5 MiB")
    }
}
