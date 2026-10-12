package org.linktolinux.app

import android.content.Intent
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.ClipDescription
import android.content.ComponentName
import android.content.pm.PackageManager
import android.provider.Settings
import android.graphics.Typeface
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.util.Log
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.TextView
import android.net.Uri
import androidx.appcompat.app.AlertDialog
import androidx.appcompat.app.AppCompatActivity
import androidx.activity.result.contract.ActivityResultContracts
import com.google.android.material.button.MaterialButton
import org.json.JSONArray
import org.json.JSONObject
import java.util.concurrent.Executors
import android.util.Base64

class MainActivity : AppCompatActivity() {
    companion object { init { System.loadLibrary("link_android_svc") } }
    external fun nativeInit(): Int
    external fun nativeLocalIdentityJson(deviceName: String, filesDir: String): String
    external fun nativeTlsPingJson(peer: String, fingerprint: String, filesDir: String): String
    external fun nativePairBegin(peer: String): String
    external fun nativePairSessions(): String
    external fun nativePairConfirm(session: String, code: String, accept: Boolean): String
    external fun nativePairsJson(): String
    external fun nativeUnpair(id: String): String
    external fun nativeClipboardEnabled(enabled: Boolean): Int
    external fun nativeClipboardStatus(): String
    external fun nativeClipboardTake(): String
    external fun nativeClipboardSend(id: String, address: String, mime: String, dataB64: String): String
    external fun nativeSendNotification(peerId: String, address: String, id: String, appName: String, title: String, body: String): String

    private lateinit var lan: LanDiscovery
    private lateinit var deviceName: String
    private val worker = Executors.newSingleThreadExecutor()
    private val main = Handler(Looper.getMainLooper())
    private val galleryPermission = registerForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) { grants ->
        status(if (grants.values.any { it }) "Gallery access granted. Paired Linux devices can browse this phone's photos." else "Gallery access was not granted. Enable photo access in Android settings to browse this phone's gallery.")
    }
    private val contactsPermission = registerForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        renderMessagesSettings()
        status(if (granted) "Contacts are available to paired Linux devices when message sharing is on" else "Contacts access was not granted")
    }
    private val messagesPermission = registerForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) { grants ->
        val granted = grants[android.Manifest.permission.READ_SMS] == true && grants[android.Manifest.permission.SEND_SMS] == true
        getSharedPreferences("link", MODE_PRIVATE).edit().putBoolean("messages_enabled", granted).apply()
        renderMessagesSettings()
        status(if (granted && checkSelfPermission(android.Manifest.permission.READ_CONTACTS) != PackageManager.PERMISSION_GRANTED) "SMS sharing enabled; phone numbers will show until contact access is allowed" else if (granted) "SMS sharing enabled for paired Linux devices" else "SMS sharing needs both read and send permissions")
    }
    private var ownId = ""
    private var nearby = emptyList<LanDiscovery.Peer>()
    private var trusted = emptyList<TrustedPeers.Peer>()
    private var sessions = JSONArray()
    private var rendered = ""
    private var busy = false
    private var visible = false
    private var pollInFlight = false
    private var clipboardEnabled = false
    private var notificationForwarding = false
    private var clipboardStateLoaded = false
    private var windowFocused = false
    private var applyingRemoteClipboard = false
    private var suppressRemoteClipboardUntil = 0L
    private var lastClipboardData: String? = null
    private var pendingNotificationClipboardSend = false
    private val clipboard by lazy { getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager }
    private val clipboardListener = ClipboardManager.OnPrimaryClipChangedListener { sampleClipboard() }
    private var pendingIntentPeer: String? = null
    private val attempted = mutableSetOf<String>()

    private val poll = object : Runnable {
        override fun run() {
            if (!visible) return
            if (!pollInFlight) {
                pollInFlight = true
                worker.execute {
                    val requests = nativePairSessions()
                    val pairs = nativePairsJson()
                    val clipStatus = JSONObject(nativeClipboardStatus())
                    val clipStatusOk = clipStatus.optBoolean("ok", false)
                    val savedClipboardEnabled = getSharedPreferences("link", MODE_PRIVATE)
                        .getBoolean("clipboard_sync", clipStatus.optBoolean("enabled", false))
                    if (clipStatusOk && savedClipboardEnabled != clipStatus.optBoolean("enabled")) {
                        nativeClipboardEnabled(savedClipboardEnabled)
                    }
                    main.post {
                        pollInFlight = false
                        if (visible && !isDestroyed) {
                            try {
                                if (!clipboardStateLoaded && clipStatusOk) {
                                    clipboardEnabled = savedClipboardEnabled
                                    clipboardStateLoaded = true
                                    findViewById<MaterialButton>(R.id.clipboardSyncBtn).text = if (clipboardEnabled) "Clipboard sync: on" else "Clipboard sync: off"
                                    if (clipboardEnabled) {
                                        clipboard.addPrimaryClipChangedListener(clipboardListener)
                                    }
                                    if (pendingNotificationClipboardSend) scheduleNotificationClipboardSend()
                                    else if (clipboardEnabled) scheduleForegroundClipboardSample()
                                }
                                val requestJson = JSONObject(requests)
                                val pairsJson = JSONObject(pairs)
                                if (requestJson.optBoolean("ok") && pairsJson.optBoolean("ok")) {
                                    sessions = requestJson.getJSONArray("sessions")
                                    trusted = TrustedPeers.parse(pairsJson.getJSONArray("pairings"))
                                    renderDevices()
                                    if (pendingIntentPeer != null && !busy) {
                                        val peer = pendingIntentPeer!!
                                        pendingIntentPeer = null
                                        connect(peer)
                                    }
                                    if (!busy) trusted.forEach { enqueueReconnect(it, addressFor(it)) }
                                }
                            } catch (e: Exception) { status("Could not load pairing requests: ${e.message}") }
                        }
                    }
                }
            }
            main.postDelayed(this, 1000)
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(R.layout.activity_main)
        pendingNotificationClipboardSend = isNotificationClipboardSend(intent)
        deviceName = phoneDisplayName()
        lan = LanDiscovery(this)
        try {
            check(nativeInit() == 0) { "Native initialization failed" }
            val identity = JSONObject(nativeLocalIdentityJson(deviceName, filesDir.absolutePath))
            ownId = identity.getString("id")
            findViewById<TextView>(R.id.identityText).text = "This device: $deviceName\n${identity.getString("pubkey_fingerprint")}" 
        } catch (e: Exception) { status("Could not load identity: ${e.message}") }
        findViewById<MaterialButton>(R.id.startServiceBtn).setOnClickListener { startLinkService() }
        findViewById<MaterialButton>(R.id.scanLanBtn).setOnClickListener { scanLan() }
        findViewById<MaterialButton>(R.id.clipboardSyncBtn).setOnClickListener {
            clipboardEnabled = !clipboardEnabled
            nativeClipboardEnabled(clipboardEnabled)
            getSharedPreferences("link", MODE_PRIVATE).edit().putBoolean("clipboard_sync", clipboardEnabled).apply()
            findViewById<MaterialButton>(R.id.clipboardSyncBtn).text = if (clipboardEnabled) "Clipboard sync: on" else "Clipboard sync: off"
            if (clipboardEnabled) {
                clipboard.addPrimaryClipChangedListener(clipboardListener)
                if (!isRemoteClipboard()) {
                    lastClipboardData = currentClipboardData()?.let { it.first + ":" + Base64.encodeToString(it.second, Base64.NO_WRAP) }
                }
                status("Clipboard sync on. It reads text and images while this app is focused.")
            } else {
                clipboard.removePrimaryClipChangedListener(clipboardListener)
                status("Clipboard sync off")
            }
        }
        findViewById<MaterialButton>(R.id.sendClipboardBtn).setOnClickListener {
            if (!clipboardEnabled) status("Enable clipboard sync on this device first")
            else sampleClipboard(force = true)
        }
        findViewById<MaterialButton>(R.id.galleryAccessBtn).setOnClickListener {
            val permissions = when {
                Build.VERSION.SDK_INT >= 34 -> arrayOf("android.permission.READ_MEDIA_IMAGES", "android.permission.READ_MEDIA_VISUAL_USER_SELECTED")
                Build.VERSION.SDK_INT >= 33 -> arrayOf("android.permission.READ_MEDIA_IMAGES")
                else -> arrayOf("android.permission.READ_EXTERNAL_STORAGE")
            }
            galleryPermission.launch(permissions)
        }
        findViewById<MaterialButton>(R.id.contactsAccessBtn).setOnClickListener {
            contactsPermission.launch(android.Manifest.permission.READ_CONTACTS)
        }
        findViewById<MaterialButton>(R.id.messagesAccessBtn).setOnClickListener {
            val prefs = getSharedPreferences("link", MODE_PRIVATE)
            if (prefs.getBoolean("messages_enabled", false)) {
                prefs.edit().putBoolean("messages_enabled", false).apply()
                renderMessagesSettings()
                status("SMS sharing disabled")
            } else if (checkSelfPermission(android.Manifest.permission.READ_SMS) == PackageManager.PERMISSION_GRANTED &&
                checkSelfPermission(android.Manifest.permission.SEND_SMS) == PackageManager.PERMISSION_GRANTED &&
                checkSelfPermission(android.Manifest.permission.READ_CONTACTS) == PackageManager.PERMISSION_GRANTED) {
                prefs.edit().putBoolean("messages_enabled", true).apply()
                renderMessagesSettings()
                status("SMS sharing enabled for paired Linux devices")
            } else {
                messagesPermission.launch(arrayOf(android.Manifest.permission.READ_SMS, android.Manifest.permission.SEND_SMS, android.Manifest.permission.READ_CONTACTS))
            }
        }
        notificationForwarding = getSharedPreferences("link", MODE_PRIVATE).getBoolean("notification_forwarding", false)
        findViewById<MaterialButton>(R.id.notificationForwardBtn).setOnClickListener {
            notificationForwarding = !notificationForwarding
            getSharedPreferences("link", MODE_PRIVATE).edit().putBoolean("notification_forwarding", notificationForwarding).apply()
            renderNotificationSettings()
            status(if (notificationForwarding) "Phone notification forwarding enabled" else "Phone notification forwarding disabled")
        }
        findViewById<MaterialButton>(R.id.notificationAccessBtn).setOnClickListener {
            startActivity(Intent(Settings.ACTION_NOTIFICATION_LISTENER_SETTINGS))
        }
        findViewById<MaterialButton>(R.id.notificationAppsBtn).setOnClickListener {
            showNotificationAppFilters()
        }
        findViewById<MaterialButton>(R.id.connectBtn).setOnClickListener {
            val address = findViewById<EditText>(R.id.peerAddress).text.toString().trim()
            val port = address.substringAfterLast(':').toIntOrNull()
            val host = address.substringBeforeLast(':', "")
            if (host.isBlank() || port == null || port !in 1..65535 || address.any { it.isWhitespace() } ||
                (host.contains(':') && !(host.startsWith('[') && host.endsWith(']')))) {
                status("Enter a host:port, for example 192.168.1.20:53536 or [::1]:53536")
            } else connect(address)
        }
        val emulator = Build.FINGERPRINT.startsWith("generic") || Build.MODEL.contains("sdk_gphone")
        findViewById<MaterialButton>(R.id.pingHostBtn).apply {
            visibility = if (emulator) android.view.View.VISIBLE else android.view.View.GONE
            setOnClickListener { connect("10.0.2.2:53536") }
        }
        if (intent?.getBooleanExtra("autoping", false) == true) pendingIntentPeer = intent.getStringExtra("peer") ?: "10.0.2.2:53536"
        // Incoming desktop requests need a live listener, so start the FGS on launch.
        startLinkService()
        status("Choose a device to pair, or enter its address. Confirm the same code on both devices.")
    }
    override fun onStart() {
        super.onStart()
        visible = true
        attempted.clear()
        scanLan()
        main.post(poll)
    }
    override fun onResume() { super.onResume(); if (::deviceName.isInitialized) { renderNotificationSettings(); renderMessagesSettings() } }
    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        if (isNotificationClipboardSend(intent)) {
            pendingNotificationClipboardSend = true
            scheduleNotificationClipboardSend()
        }
    }
    override fun onStop() {
        visible = false
        main.removeCallbacks(poll)
        lan.stopBrowsing()
        super.onStop()
    }
    override fun onDestroy() { clipboard.removePrimaryClipChangedListener(clipboardListener); worker.shutdown(); super.onDestroy() }
    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        windowFocused = hasFocus
        if (hasFocus) {
            if (pendingNotificationClipboardSend) scheduleNotificationClipboardSend()
            else if (clipboardEnabled) scheduleForegroundClipboardSample()
        }
    }
    private fun isNotificationClipboardSend(intent: Intent?): Boolean {
        if (intent?.action != LinkService.ACTION_SEND_CLIPBOARD) return false
        val expectedToken = getSharedPreferences("link", MODE_PRIVATE)
            .getString(LinkService.PREF_NOTIFICATION_TOKEN, null)
        return expectedToken != null && intent.getStringExtra(LinkService.EXTRA_NOTIFICATION_TOKEN) == expectedToken
    }
    private fun scheduleNotificationClipboardSend() {
        if (!pendingNotificationClipboardSend || !visible || !windowFocused || !clipboardStateLoaded) return
        if (!clipboardEnabled) {
            pendingNotificationClipboardSend = false
            status("Turn on clipboard sync before sending a clip")
            return
        }
        pendingNotificationClipboardSend = false
        // The notification action first brings this activity to the foreground.
        // Delay the read until Android grants clipboard access to the focused app.
        main.postDelayed({
            if (visible && windowFocused && clipboardEnabled && !isDestroyed) {
                sampleClipboard(force = true)
            }
        }, 1500)
    }
    private fun scheduleForegroundClipboardSample() {
        // Android may notify us about a clipboard change while this activity
        // is backgrounded, when clipboard reads are restricted. Re-sample
        // after the foreground poll can apply any incoming peer clip.
        main.postDelayed({
            if (visible && windowFocused && clipboardEnabled && !isDestroyed) {
                sampleClipboard()
            }
        }, 1500)
    }
    private fun status(message: String) {findViewById<TextView>(R.id.statusText).text = message}
    private fun renderMessagesSettings() {
        findViewById<MaterialButton>(R.id.contactsAccessBtn).text = if (checkSelfPermission(android.Manifest.permission.READ_CONTACTS) == PackageManager.PERMISSION_GRANTED) "Contacts access: allowed" else "Allow contact access"
        val permissionsGranted = checkSelfPermission(android.Manifest.permission.READ_SMS) == PackageManager.PERMISSION_GRANTED &&
            checkSelfPermission(android.Manifest.permission.SEND_SMS) == PackageManager.PERMISSION_GRANTED
        val enabled = getSharedPreferences("link", MODE_PRIVATE).getBoolean("messages_enabled", false) && permissionsGranted
        findViewById<MaterialButton>(R.id.messagesAccessBtn).text = if (enabled) "Message sharing: on · tap to turn off" else if (permissionsGranted) "Enable message sharing" else "Allow SMS read and send access"
    }
    private fun renderNotificationSettings() {
        findViewById<MaterialButton>(R.id.notificationForwardBtn).text = if (notificationForwarding) "Forwarding: on" else "Forwarding: off"
        val enabled = getSystemService(android.app.NotificationManager::class.java)
            .isNotificationListenerAccessGranted(ComponentName(this, LinkNotifications::class.java))
        findViewById<MaterialButton>(R.id.notificationAccessBtn).text = if (enabled) "Notification access: allowed" else "Grant notification access"
    }
    private fun showNotificationAppFilters() {
        val prefs = getSharedPreferences("link", MODE_PRIVATE)
        val apps = try {
            val json = JSONObject(prefs.getString("notification_seen_apps", "{}") ?: "{}")
            (0 until json.length()).map { index ->
                val packageName = json.names()?.get(index)?.toString() ?: return@map null
                packageName to json.getString(packageName)
            }.filterNotNull().sortedBy { it.second.lowercase() }
        } catch (_: Exception) { emptyList() }
        if (apps.isEmpty()) {
            AlertDialog.Builder(this)
                .setTitle("Notification apps")
                .setMessage("Apps will appear here after they post a non-ongoing notification while Link has notification access.")
                .setPositiveButton("OK", null)
                .show()
            return
        }
        val blocked = prefs.getStringSet("notification_blocked_apps", emptySet())?.toMutableSet() ?: mutableSetOf()
        val selected = BooleanArray(apps.size) { index -> apps[index].first !in blocked }
        AlertDialog.Builder(this)
            .setTitle("Forward notifications from")
            .setMultiChoiceItems(apps.map { it.second }.toTypedArray(), selected) { _, which, checked ->
                selected[which] = checked
            }
            .setPositiveButton("Save") { _, _ ->
                val updatedBlocked = apps.indices
                    .filterNot { selected[it] }
                    .map { apps[it].first }
                    .toSet()
                prefs.edit().putStringSet("notification_blocked_apps", updatedBlocked).apply()
                status("Notification app filters saved")
            }
            .setNegativeButton("Cancel", null)
            .show()
    }
    private fun currentClipboardData(): Pair<String, ByteArray>? {
      return try {
        val description = clipboard.primaryClipDescription
        if (!clipboard.hasPrimaryClip() || description == null ||
            (Build.VERSION.SDK_INT >= 24 && description.extras?.getBoolean("android.content.extra.IS_SENSITIVE", false) == true)) return null
        val item = clipboard.primaryClip?.getItemAt(0) ?: return null
        val advertisedTypes = (0 until description.mimeTypeCount).map(description::getMimeType)
        val imageUri = item.uri ?: item.intent?.data ?: if (
            advertisedTypes.any { it.startsWith("image/") }
        ) {
            item.text?.toString()?.takeIf { it.startsWith("content://") || it.startsWith("file://") }
                ?.let(android.net.Uri::parse)
        } else null
        val providerMime = imageUri?.let { runCatching { contentResolver.getType(it) }.getOrNull() }
        val isImageClip = providerMime?.startsWith("image/") == true ||
            advertisedTypes.any { it.startsWith("image/") }
        if (imageUri != null && isImageClip) {
            val bytes = contentResolver.openInputStream(imageUri)?.use { input ->
                val output = java.io.ByteArrayOutputStream()
                val buffer = ByteArray(8192)
                var total = 0
                while (true) {
                    val count = input.read(buffer)
                    if (count < 0) break
                    total += count
                    if (total > 5 * 1024 * 1024) return null
                    output.write(buffer, 0, count)
                }
                output.toByteArray()
            } ?: return null
            // Clipboard providers disagree about MIME metadata (some report
            // image/* or text/uri-list). Detect the encoded payload itself so
            // the Linux peer receives the MIME type matching the image bytes.
            val actualMime = detectImageMime(bytes) ?: return null
            actualMime to bytes
        } else if (description.hasMimeType(ClipDescription.MIMETYPE_TEXT_PLAIN)) {
            val bytes = item.text?.toString()?.toByteArray(Charsets.UTF_8) ?: return null
            if (bytes.size <= 32 * 1024) "text/plain" to bytes else null
        } else null
      } catch (_: Exception) { null }
    }
    private fun detectImageMime(bytes: ByteArray): String? = when {
        bytes.size >= 8 && bytes.copyOfRange(0, 8).contentEquals(byteArrayOf(
            0x89.toByte(), 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a
        )) -> "image/png"
        bytes.size >= 3 && bytes[0] == 0xff.toByte() && bytes[1] == 0xd8.toByte() && bytes[2] == 0xff.toByte() -> "image/jpeg"
        bytes.size >= 12 && String(bytes, 0, 4, Charsets.US_ASCII) == "RIFF" &&
            String(bytes, 8, 4, Charsets.US_ASCII) == "WEBP" -> "image/webp"
        else -> null
    }
    private fun isRemoteClipboard() = clipboard.primaryClipDescription?.label?.toString()?.startsWith("Link to Linux") == true
    private fun sampleClipboard(force: Boolean = false) {
        if (!clipboardEnabled || !visible || !hasWindowFocus() || applyingRemoteClipboard) return
        if (!force && android.os.SystemClock.uptimeMillis() < suppressRemoteClipboardUntil) return
        // Ignore our own remote clip on Android's asynchronous clipboard-change callback.
        // The explicit button still permits a deliberate resend.
        val remoteClip = isRemoteClipboard()
        if (!force && remoteClip) return
        val cachedRemote = if (force && remoteClip) lastClipboardData else null
        val captured = cachedRemote?.let { encoded ->
            val separator = encoded.indexOf(':')
            if (separator > 0) {
                val mime = encoded.substring(0, separator)
                try { mime to Base64.decode(encoded.substring(separator + 1), Base64.NO_WRAP) }
                catch (_: IllegalArgumentException) { null }
            } else null
        } ?: currentClipboardData()
        if (captured == null) {
            if (force) main.post {
                if (visible && !isDestroyed) status("No readable clipboard item found. Open the app before copying, then use Send current.")
            }
            return
        }
        val (mime, data) = captured
        // Some Android clipboard providers report a received image URI as an
        // empty text item in their delayed change callback. Never echo that
        // fallback over the actual image payload.
        // Android providers can briefly expose an image URI as an empty text
        // clip. Empty text is not useful clipboard content and can otherwise
        // replace a valid image at the paired peer.
        if (mime == "text/plain" && data.isEmpty()) return
        val encoded = Base64.encodeToString(data, Base64.NO_WRAP)
        if (!force && mime + ":" + encoded == lastClipboardData) return
        lastClipboardData = mime + ":" + encoded
        val sentMime = mime
        worker.execute {
            var sent = false
            trusted.forEach { peer ->
                try { val result = JSONObject(nativeClipboardSend(peer.id, addressFor(peer), mime, encoded)); if (!result.optBoolean("ok")) Log.w("LinkToLinux", "clipboard send rejected") else sent = true }
                catch (e: Exception) { Log.w("LinkToLinux", "clipboard send failed: ${e.message}") }
            }
            if (force) main.post { if (visible && !isDestroyed) status(if (sent) "Current clipboard sent · $sentMime" else "Could not send clipboard to trusted devices") }
        }
    }
    private fun applyRemoteClipboard(mime: String, encoded: String) {
        if (!clipboardEnabled || !windowFocused) return
        val bytes = try { Base64.decode(encoded, Base64.NO_WRAP) } catch (_: IllegalArgumentException) { return }
        if ((mime == "text/plain" && bytes.size > 32 * 1024) || (mime.startsWith("image/") && bytes.size > 5 * 1024 * 1024)) return
        if (mime == "text/plain" && bytes.isEmpty()) return
        applyingRemoteClipboard = true
        suppressRemoteClipboardUntil = android.os.SystemClock.uptimeMillis() + 1500
        lastClipboardData = mime + ":" + encoded
        if (mime == "text/plain") {
            clipboard.setPrimaryClip(ClipData.newPlainText("Link to Linux", bytes.toString(Charsets.UTF_8)))
        } else if (mime in listOf("image/png", "image/jpeg", "image/webp")) {
            val uri = ClipboardImageProvider.publish(this, mime, bytes)
            // newUri advertises the provider's image MIME type to paste targets.
            clipboard.setPrimaryClip(ClipData.newUri(contentResolver, "Link to Linux image", uri))
        } else { applyingRemoteClipboard = false; return }
        main.post { applyingRemoteClipboard = false; status("Clipboard received · $mime") }
    }
    private fun runWork(work: () -> String, complete: (JSONObject) -> Unit) {
        if (busy) {status("A request is already in progress"); return}
        busy = true
        worker.execute {
            val response = try {JSONObject(work())} catch (e: Exception) {JSONObject().put("ok", false).put("error", e.message)}
            main.post {
                busy = false
                if (visible && !isFinishing && !isDestroyed) {
                    if (response.optBoolean("ok")) complete(response) else status("Request failed: ${response.optString("error")}")
                }
            }
        }
    }
    private fun scanLan() {
        val summary = findViewById<TextView>(R.id.discoveryStatus)
        summary.text = "Looking for devices on your LAN…"
        lan.browse(ownId, {peers ->
            nearby = peers
            summary.text = if (peers.isEmpty()) "No nearby devices yet. You can also enter an address above." else "${peers.size} nearby device(s)"
            renderDevices()
        }, {summary.text = it})
    }
    private fun addressFor(saved: TrustedPeers.Peer): String {
        nearby.find {it.id == saved.id}?.let {return it.address}
        // ADB-forwarded incoming connections originate at emulator loopback.
        val emulator = Build.FINGERPRINT.startsWith("generic") || Build.MODEL.contains("sdk_gphone")
        if (emulator && saved.address.startsWith("127.0.0.1:")) return "10.0.2.2:${saved.address.substringAfterLast(':')}"
        return saved.address
    }
    private fun renderDevices() {
        val stableSessions = JSONArray(sessions.toString())
        for (index in 0 until stableSessions.length()) stableSessions.getJSONObject(index).remove("expires_in_secs")
        val signature = stableSessions.toString() + trusted.toString() + nearby.toString()
        if (rendered == signature) return
        rendered = signature
        val list = findViewById<LinearLayout>(R.id.devicesList)
        list.removeAllViews()
        for (index in 0 until sessions.length()) {
            val session = sessions.getJSONObject(index)
            val peer = session.getJSONObject("peer")
            val state = session.getString("status")
            val card = deviceCard()
            val content = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL }
            content.addView(label("Pairing with ${peer.getString("name")}", 17f, true))
            if (state == "awaiting" || state == "waiting") {
                val code = session.getString("code")
                content.addView(label(code, 34f, true, R.color.link_primary))
                content.addView(label("Compare this code on both devices. This request expires after two minutes.", 13f, false, R.color.link_muted))
                if (state == "awaiting") {
                    content.addView(MaterialButton(this).apply {
                        text = "Codes match — confirm"
                        setOnClickListener { runWork({nativePairConfirm(session.getString("session"), code, true)}) {status("Confirmed here. Confirm the matching code on the other device.")} }
                    })
                    content.addView(MaterialButton(this).apply {
                        text = "Reject"
                        setOnClickListener {runWork({nativePairConfirm(session.getString("session"), code, false)}) {status("Pairing rejected")}}
                    })
                } else content.addView(label("Confirmed here. Waiting for the other device…", 13f, false, R.color.link_muted))
            } else content.addView(label(when (state) {"paired" -> "Paired successfully"; "expired" -> "Code expired. Start a new request."; "rejected" -> "Pairing rejected"; else -> "Pairing failed"}, 13f, false, R.color.link_muted))
            card.addView(content)
            list.addView(card)
        }
        trusted.forEach {saved ->
            val address = addressFor(saved)
            val card = deviceCard()
            val content = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL }
            content.addView(label(saved.name, 16f, true))
            content.addView(label("Trusted · $address", 12f, false, R.color.link_muted))
            content.addView(MaterialButton(this).apply {text = "Check connection"; setOnClickListener {ping(saved, address)}})
            content.addView(MaterialButton(this).apply {
                text = "Unpair"
                setOnClickListener {
                    AlertDialog.Builder(this@MainActivity).setTitle("Unpair ${saved.name}?")
                        .setMessage("Clipboard and notification sharing with this device will stop.")
                        .setNegativeButton("Cancel", null).setPositiveButton("Unpair") {_, _ -> runWork({nativeUnpair(saved.id)}) {status("Device unpaired")} }.show()
                }
            })
            card.addView(content)
            list.addView(card)
        }
        nearby.filter {peer -> trusted.none {it.id == peer.id}}.forEach {peer ->
            val card = deviceCard()
            val content = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL }
            content.addView(label(peer.name, 16f, true))
            content.addView(label("${peer.os} · ${peer.address}", 12f, false, R.color.link_muted))
            content.addView(MaterialButton(this).apply {text = "Pair device"; setOnClickListener {connect(peer.address, peer.id)}})
            card.addView(content)
            list.addView(card)
        }
    }
    private fun label(value: String, size: Float, bold: Boolean, color: Int = R.color.link_text) = TextView(this).apply {
        text = value
        textSize = size
        setTextColor(getColor(color))
        if (bold) setTypeface(typeface, Typeface.BOLD)
        setPadding(0, dp(5), 0, dp(5))
    }
    private fun deviceCard() = com.google.android.material.card.MaterialCardView(this).apply {
        radius = dp(14).toFloat()
        setCardBackgroundColor(getColor(R.color.link_surface))
        strokeColor = getColor(R.color.link_primary)
        strokeWidth = dp(1)
        useCompatPadding = true
        setContentPadding(dp(14), dp(10), dp(14), dp(10))
        val params = LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT, LinearLayout.LayoutParams.WRAP_CONTENT)
        params.bottomMargin = dp(8)
        layoutParams = params
    }
    private fun dp(value: Int) = (value * resources.displayMetrics.density).toInt()
    private fun connect(address: String, expectedId: String? = null) {
        val saved = trusted.find {it.id == expectedId || addressFor(it) == address}
        if (saved != null) {ping(saved, address); return}
        status("Requesting pairing…")
        runWork({nativePairBegin(address)}) {result ->
            val pairing = result.getJSONObject("pairing")
            if (expectedId != null && pairing.getJSONObject("peer").getString("id") != expectedId) {
                runWork({nativePairConfirm(pairing.getString("session"), "", false)}) {status("Discovered device identity changed; request rejected")}
            } else status("Compare code ${pairing.getString("code")} on both devices and confirm on each.")
        }
    }
    private fun ping(saved: TrustedPeers.Peer, address: String) {
        status("Connecting to ${saved.name}…")
        runWork({nativeTlsPingJson(address, saved.fingerprint, filesDir.absolutePath)}) {result ->
            if (result.optString("pong_from") == saved.id) status("Connected to ${saved.name} over TLS") else status("Peer identity changed; unpair before trusting it again")
        }
    }
    private fun enqueueReconnect(saved: TrustedPeers.Peer, address: String) {
        if (!attempted.add("${saved.id}@$address")) return
        worker.execute {
            val response = nativeTlsPingJson(address, saved.fingerprint, filesDir.absolutePath)
            Log.i("LinkToLinux", "reconnect $address -> $response")
            main.post {
                if (visible && !isDestroyed && !busy) {
                    try {val result = JSONObject(response); if (result.optBoolean("ok") && result.optString("pong_from") == saved.id) status("Connected to ${saved.name} over TLS")}
                    catch (_: Exception) { /* Manual Ping displays errors. */ }
                }
            }
        }
    }
    private fun startLinkService() {startForegroundService(Intent(this, LinkService::class.java))}
}
