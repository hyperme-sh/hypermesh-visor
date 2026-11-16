package sh.hyperme.visor.mobile.tunnel

import android.content.Context
import android.content.Intent
import android.net.VpnService
import android.os.SystemClock
import com.wireguard.android.backend.GoBackend
import com.wireguard.android.backend.Tunnel
import com.wireguard.config.Config
import com.wireguard.crypto.Key

/** Optional app-owned adapter. It never reads keys from files or starts at boot. */
class AndroidMeshTunnel(context: Context) {
    private val app = context.applicationContext
    private val backend = GoBackend(app)
    private val monitor = TunnelMonitor()
    private var peer: Key? = null
    private val tunnel = object : Tunnel {
        override fun getName() = "visor-mesh"
        override fun onStateChange(state: Tunnel.State) { /* Every status read probes the backend. */ }
    }

    /** The foreground UI owns showing this intent and handling denial. */
    fun permissionRequest(): Intent? = VpnService.prepare(app)

    /** Call off the UI thread, after the enrollment owner supplies an in-memory config. */
    @Synchronized fun start(config: Config): Boolean {
        // Stop first: invalid replacement configuration must not retain the old tunnel.
        if (!stop()) return false
        val iface = config.`interface`
        val selected = config.peers.singleOrNull() ?: return false
        if (VpnService.prepare(app) != null || iface.addresses.isEmpty() ||
            iface.includedApplications != setOf(app.packageName) || iface.excludedApplications.isNotEmpty() ||
            iface.listenPort.orElse(0) != 0 || selected.allowedIps.isEmpty() ||
            !selected.endpoint.isPresent || selected.persistentKeepalive.orElse(0) !in 1..25) return false
        monitor.begin(System.currentTimeMillis(), SystemClock.elapsedRealtime())
        peer = selected.publicKey
        return try {
            if (backend.setState(tunnel, Tunnel.State.UP, config) != Tunnel.State.UP) {
                stop()
                monitor.failed()
                false
            } else true
        } catch (_: Exception) {
            stop()
            monitor.failed()
            false
        }
    }

    @Synchronized fun stop(): Boolean {
        monitor.stop()
        peer = null
        val stopped = try { backend.setState(tunnel, Tunnel.State.DOWN, null) == Tunnel.State.DOWN }
        catch (_: Exception) { false }
        if (!stopped) monitor.failed()
        return stopped
    }

    /** CONNECTED means a fresh handshake with the configured peer, not merely an installed VPN. */
    @Synchronized fun status(): TunnelStatus {
        return try {
            val selected = peer
            val up = selected != null && backend.getState(tunnel) == Tunnel.State.UP
            val handshake = if (up) backend.getStatistics(tunnel).peer(selected!!)?.latestHandshakeEpochMillis() ?: 0L else 0L
            monitor.sample(up, handshake, System.currentTimeMillis(), SystemClock.elapsedRealtime())
        } catch (_: Exception) { monitor.sample(false, 0, System.currentTimeMillis(), SystemClock.elapsedRealtime()) }
    }
}
