package org.linktolinux.app

import android.content.Context
import android.os.Build
import android.provider.Settings

/** Android's user-configured device name, without exposing its hardware model as a label. */
fun Context.phoneDisplayName(): String {
    val candidates = listOf(
        runCatching { Settings.Global.getString(contentResolver, "device_name") }.getOrNull(),
        runCatching { Settings.Secure.getString(contentResolver, "bluetooth_name") }.getOrNull(),
    )
    return candidates.firstNotNullOfOrNull { candidate ->
        candidate?.trim()?.takeIf { name ->
            name.isNotEmpty() &&
                !name.equals(Build.MODEL, ignoreCase = true) &&
                !name.equals("${Build.MANUFACTURER} ${Build.MODEL}", ignoreCase = true) &&
                !name.contains("sdk_gphone", ignoreCase = true)
        }
    } ?: "Android phone"
}
