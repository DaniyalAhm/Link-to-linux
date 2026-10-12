package org.linktolinux.app

import android.content.Context
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.os.Handler
import android.os.Looper
import org.json.JSONObject
import java.util.UUID

/** Android DNS-SD interoperates with link-core's mDNS records. All state is on main. */
class LanDiscovery(context: Context) {
    companion object { const val TYPE = "_linktolinux._udp." }
    data class Peer(val id: String, val name: String, val os: String, val address: String)
    private val nsd = context.getSystemService(NsdManager::class.java)
    private val main = Handler(Looper.getMainLooper())
    private var discovery: NsdManager.DiscoveryListener? = null
    private var registration: NsdManager.RegistrationListener? = null
    private var generation = 0

    fun browse(ownId: String, changed: (List<Peer>) -> Unit, error: (String) -> Unit) {
        stopBrowsing()
        val epoch = generation
        val peers = linkedMapOf<String, Peer>()
        val pending = linkedMapOf<String, NsdServiceInfo>()
        val present = mutableSetOf<String>()
        var resolving = false
        fun publish() = changed(peers.values.distinctBy { it.id }.sortedBy { it.name })
        fun resolveNext() {
            if (epoch != generation || resolving || pending.isEmpty()) return
            val entry = pending.entries.first()
            val key = entry.key
            val service = entry.value
            pending.remove(key)
            resolving = true
            @Suppress("DEPRECATION")
            try {
                nsd.resolveService(service, object : NsdManager.ResolveListener {
                    override fun onResolveFailed(info: NsdServiceInfo, code: Int) {
                        main.post {
                            if (epoch == generation) {
                                resolving = false
                                error("Could not resolve ${info.serviceName} ($code)")
                                resolveNext()
                            }
                        }
                    }
                    override fun onServiceResolved(info: NsdServiceInfo) {
                        main.post {
                            if (epoch != generation) return@post
                            resolving = false
                            fun attr(name: String) = info.attributes[name]?.toString(Charsets.UTF_8)
                            val id = attr("id")
                            @Suppress("DEPRECATION")
                            val host = info.host?.hostAddress
                            val validId = runCatching { UUID.fromString(id) }.isSuccess
                            if (key in present && validId && id != ownId && attr("proto") == "1" &&
                                host != null && info.port in 1..65535) {
                                val address = if (host.contains(':')) "[$host]:${info.port}" else "$host:${info.port}"
                                peers[key] = Peer(id!!, info.serviceName.substringBeforeLast('-'), attr("os") ?: "unknown", address)
                                publish()
                            }
                            resolveNext()
                        }
                    }
                })
            } catch (e: RuntimeException) {
                resolving = false
                error("Could not resolve device: ${e.message}")
                main.post { resolveNext() }
            }
        }
        val listener = object : NsdManager.DiscoveryListener {
            override fun onDiscoveryStarted(type: String) = Unit
            override fun onDiscoveryStopped(type: String) = Unit
            override fun onStartDiscoveryFailed(type: String, code: Int) {
                main.post {
                    if (epoch == generation) {
                        stopBrowsing()
                        error("LAN discovery failed ($code). Tap Scan LAN to retry.")
                    }
                }
            }
            override fun onStopDiscoveryFailed(type: String, code: Int) {
                main.post { if (epoch == generation) error("Could not stop discovery ($code)") }
            }
            override fun onServiceFound(info: NsdServiceInfo) {
                main.post {
                    if (epoch == generation && info.serviceType.trimEnd('.') == TYPE.trimEnd('.')) {
                        present.add(info.serviceName)
                        pending[info.serviceName] = info
                        resolveNext()
                    }
                }
            }
            override fun onServiceLost(info: NsdServiceInfo) {
                main.post {
                    if (epoch == generation) {
                        present.remove(info.serviceName)
                        pending.remove(info.serviceName)
                        peers.remove(info.serviceName)
                        publish()
                    }
                }
            }
        }
        discovery = listener
        changed(emptyList())
        try { nsd.discoverServices(TYPE, NsdManager.PROTOCOL_DNS_SD, listener) }
        catch (e: RuntimeException) { stopBrowsing(); error("LAN discovery failed: ${e.message}") }
    }

    fun advertise(identity: JSONObject, error: (String) -> Unit) {
        val listener = object : NsdManager.RegistrationListener {
            override fun onServiceRegistered(info: NsdServiceInfo) = Unit
            override fun onServiceUnregistered(info: NsdServiceInfo) = Unit
            override fun onRegistrationFailed(info: NsdServiceInfo, code: Int) {
                main.post {
                    if (registration === this) { registration = null; error("LAN advertisement failed ($code)") }
                }
            }
            override fun onUnregistrationFailed(info: NsdServiceInfo, code: Int) {
                main.post { error("LAN advertisement cleanup failed ($code)") }
            }
        }
        val info = NsdServiceInfo().apply {
            serviceName = "${identity.getString("name").take(40)}-${identity.getString("id").take(8)}"
            serviceType = TYPE
            port = identity.getInt("quic_port")
            setAttribute("id", identity.getString("id"))
            setAttribute("os", "android")
            setAttribute("proto", identity.getInt("protocol_version").toString())
            setAttribute("fp", identity.getString("pubkey_fingerprint"))
        }
        registration = listener
        try { nsd.registerService(info, NsdManager.PROTOCOL_DNS_SD, listener) }
        catch (e: RuntimeException) { registration = null; error("LAN advertisement failed: ${e.message}") }
    }

    fun stopBrowsing() {
        generation++
        discovery?.let { runCatching { nsd.stopServiceDiscovery(it) } }
        discovery = null
    }

    fun close() {
        stopBrowsing()
        registration?.let { runCatching { nsd.unregisterService(it) } }
        registration = null
    }
}
