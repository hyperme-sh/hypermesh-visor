package sh.hyperme.visor.mobile.tunnel

/** A recent handshake establishes peer reachability, never service authorization. */
enum class TunnelStatus { DISCONNECTED, CONNECTING, CONNECTED, RECONNECTING, UNREACHABLE }

internal class TunnelMonitor {
    private var enabled = false
    private var startedWall = 0L
    private var startedMono = 0L
    private var previousWall = 0L
    private var previousMono = 0L
    private var clockAnchor = 0L
    private var reached = false
    private var clockFailed = false

    @Synchronized fun begin(wall: Long, mono: Long) {
        enabled = true
        // WireGuard handshake timestamps may have whole-second precision.
        startedWall = wall - wall % 1_000
        startedMono = mono
        previousWall = wall
        previousMono = mono
        clockAnchor = wall
        reached = false
        clockFailed = wall <= 0 || mono < 0
    }

    @Synchronized fun stop() { enabled = false }
    @Synchronized fun failed() { enabled = true; clockFailed = true }

    @Synchronized fun sample(up: Boolean, latestHandshake: Long, wall: Long, mono: Long): TunnelStatus {
        if (!enabled) return TunnelStatus.DISCONNECTED
        if (wall < previousWall || mono < previousMono) clockFailed = true
        previousWall = wall
        previousMono = mono
        if (clockFailed) return TunnelStatus.UNREACHABLE
        if (!up) {
            // A previous native tunnel must not supply authority after a new start.
            startedWall = wall
            return TunnelStatus.UNREACHABLE
        }
        if (latestHandshake > wall) return TunnelStatus.UNREACHABLE
        // A frozen or slowed wall clock cannot renew a handshake on later polls.
        val effectiveWall = maxOf(wall, clockAnchor + (mono - startedMono))
        if (latestHandshake >= startedWall && latestHandshake > 0 && effectiveWall - latestHandshake <= 180_000) {
            reached = true
            return TunnelStatus.CONNECTED
        }
        if (reached) return TunnelStatus.RECONNECTING
        return if (mono - startedMono <= 30_000) TunnelStatus.CONNECTING else TunnelStatus.UNREACHABLE
    }
}
