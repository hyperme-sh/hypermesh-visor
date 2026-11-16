import Foundation

public enum MeshTunnelStatus: String {
    case disconnected, connecting, connected, reconnecting, unreachable
}

final class TunnelMonitor {
    private var enabled = false
    private var startWall: Int64 = 0
    private var startMono: Int64 = 0
    private var previousWall: Int64 = 0
    private var previousMono: Int64 = 0
    private var effectiveWall: Int64 = 0
    private var reached = false
    private var clockFailed = false

    // The adapter serializes all access on its own queue.
    func begin(wall: Int64, mono: Int64) {
        enabled = true
        startWall = wall - wall % 1000
        startMono = mono
        previousWall = wall
        previousMono = mono
        effectiveWall = wall
        reached = false
        clockFailed = wall <= 0 || mono < 0
    }
    func stop() { enabled = false }
    func failed() { enabled = true; clockFailed = true }
    func sample(up: Bool, handshake: Int64, wall: Int64, mono: Int64) -> MeshTunnelStatus {
        guard enabled else { return .disconnected }
        if wall < previousWall || mono < previousMono { clockFailed = true }
        guard !clockFailed else { return .unreachable }
        let elapsed = mono - previousMono
        previousWall = wall
        previousMono = mono
        let (next, overflow) = effectiveWall.addingReportingOverflow(elapsed)
        if overflow { clockFailed = true; return .unreachable }
        effectiveWall = max(wall, next)
        guard up else { startWall = wall - wall % 1000; return .unreachable }
        guard handshake <= wall else { return .unreachable }
        if handshake >= startWall && handshake > 0 && effectiveWall - handshake <= 180_000 {
            reached = true
            return .connected
        }
        if reached { return .reconnecting }
        return mono - startMono <= 30_000 ? .connecting : .unreachable
    }
}

// Extract one bounded field for the exact configured peer; never return raw settings.
func peerHandshake(_ settings: String, peer: String) -> Int64? {
    guard settings.utf8.count <= 65_536 else { return nil }
    var selected = false
    var matched = false
    var value: Int64?
    for line in settings.split(separator: "\n") {
        if line.hasPrefix("public_key=") {
            selected = line.dropFirst(11) == peer
            if selected {
                guard !matched else { return nil }
                matched = true
            }
        } else if selected && line.hasPrefix("last_handshake_time_sec=") {
            guard value == nil, let seconds = Int64(line.dropFirst(24)), seconds >= 0,
                  seconds <= Int64.max / 1000 else { return nil }
            value = seconds * 1000
        }
    }
    return matched ? value : nil
}

// Generation fencing is shared by the native callbacks and the unit contract.
final class TunnelOperations {
    enum Phase { case stopped, starting, running, stopping, failed }
    private(set) var phase: Phase = .stopped
    private var generation = UUID()
    private let monitor = TunnelMonitor()
    var stamp: UUID { generation }
    func current(_ stamp: UUID) -> Bool { generation == stamp }
    func start(wall: Int64, mono: Int64) -> UUID? {
        guard phase == .stopped else { return nil }
        generation = UUID()
        phase = .starting
        monitor.begin(wall: wall, mono: mono)
        return generation
    }
    func started(_ stamp: UUID, success: Bool) -> Bool {
        guard current(stamp), phase == .starting else { return false }
        phase = success ? .running : .failed
        if !success { monitor.failed() }
        return true
    }
    func stop() -> UUID {
        generation = UUID()
        phase = .stopping
        monitor.failed()
        return generation
    }
    func stopped(_ stamp: UUID, success: Bool) -> Bool {
        guard current(stamp), phase == .stopping else { return false }
        phase = success ? .stopped : .failed
        if success { monitor.stop() }
        return true
    }
    func status(up: Bool, handshake: Int64, wall: Int64, mono: Int64) -> MeshTunnelStatus {
        switch phase {
        case .stopped: return .disconnected
        case .stopping, .failed: return .unreachable
        case .starting: return monitor.sample(up: true, handshake: 0, wall: wall, mono: mono)
        case .running: return monitor.sample(up: up, handshake: handshake, wall: wall, mono: mono)
        }
    }
}

// Both interface addresses and peer networks become included routes upstream.
func validTunnelPrefixes(interface: [UInt8], peer: [UInt8]) -> Bool {
    !interface.isEmpty && interface.count <= 8 && interface.allSatisfy { $0 > 0 }
        && !peer.isEmpty && peer.count <= 32 && peer.allSatisfy { $0 > 0 }
}
