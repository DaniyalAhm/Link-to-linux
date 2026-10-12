package org.linktolinux.app

import org.json.JSONArray
import org.json.JSONTokener
import java.io.File

/** Rust persists trust only after both users confirm the session code. */
object TrustedPeers {
    data class Peer(val id: String, val name: String, val address: String, val fingerprint: String)
    fun parse(entries: JSONArray): List<Peer> = (0 until entries.length()).map { index ->
        val item = entries.getJSONObject(index)
        Peer(item.getString("id"), item.getString("name"),
            item.optString("address", item.getString("name")), item.getString("cert_fingerprint"))
    }

    fun load(filesDir: File): List<Peer> = try {
        val entries = JSONTokener(File(filesDir, "paired.json").readText()).nextValue() as JSONArray
        parse(entries)
    } catch (_: Exception) { emptyList() }
}
