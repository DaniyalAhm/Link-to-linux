package org.linktolinux.app

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.*
import org.junit.Test

/** Explicit live-device clipboard check. A host harness supplies unique fixture tokens. */
class ClipboardRoundTripTest {
    @Test fun phoneToDesktopAndBack() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val args = InstrumentationRegistry.getArguments()
        val outbound = requireNotNull(args.getString("outbound")) { "Requires host clipboard harness" }
        val inbound = requireNotNull(args.getString("inbound")) { "Requires host clipboard harness" }
        val context = instrumentation.targetContext
        assertTrue("Enable clipboard sync on the phone first", context.getSharedPreferences("link", Context.MODE_PRIVATE).getBoolean("clipboard_sync", false))
        val activity = instrumentation.startActivitySync(Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
        val clipboard = context.getSystemService(ClipboardManager::class.java)
        var original: ClipData? = null
        var clipboardCaptured = false
        try {
            val focusDeadline = android.os.SystemClock.elapsedRealtime() + 15000
            while (!activity.hasWindowFocus() && android.os.SystemClock.elapsedRealtime() < focusDeadline) Thread.sleep(100)
            assertTrue("Unlock the phone and leave Link in the foreground", activity.hasWindowFocus())
            // Allow Link's initial settings and pairing snapshot to finish loading.
            Thread.sleep(2500)
            instrumentation.runOnMainSync {
                original = clipboard.primaryClip
                clipboardCaptured = true
                clipboard.setPrimaryClip(ClipData.newPlainText("Link sync test", outbound))
            }
            val deadline = android.os.SystemClock.elapsedRealtime() + 60000
            var matched = false
            while (!matched && android.os.SystemClock.elapsedRealtime() < deadline) {
                instrumentation.runOnMainSync { matched = clipboard.primaryClip?.getItemAt(0)?.text?.toString() == inbound }
                if (!matched) Thread.sleep(250)
            }
            assertTrue("Desktop reply did not reach the Android clipboard", matched)
        } finally {
            instrumentation.runOnMainSync {
                if (clipboardCaptured) {
                    original?.let { clipboard.setPrimaryClip(it) } ?: clipboard.clearPrimaryClip()
                }
            }
        }
    }
}
