import Foundation
import Darwin
import NetworkExtension
import WireGuardKit

/// App-owned adapter for a NetworkExtension provider. No storage, listener or auto-start.
public final class AppleMeshTunnel {
    private let adapter: WireGuardAdapter
    private let queue = DispatchQueue(label: "sh.hyperme.visor.mesh")
    private let operations = TunnelOperations()
    private var peer: String?

    public init(provider: NEPacketTunnelProvider) {
        // Upstream diagnostics can contain private topology. Do not forward them.
        adapter = WireGuardAdapter(with: provider, logHandler: { _, _ in })
    }

    /// The enrolled-device owner supplies in-memory configuration from its secure store.
    /// Call stop and await its result before replacing a configuration.
    public func start(configuration: TunnelConfiguration, completion: @escaping (Bool) -> Void) {
        guard configuration.peers.count == 1 else { completion(false); return }
        let copy = TunnelConfiguration(name: nil, interface: configuration.interface, peers: configuration.peers)
        queue.async {
            guard self.operations.phase == .stopped,
                  (copy.interface.listenPort ?? 0) == 0,
                  let selected = copy.peers.first, selected.endpoint != nil,
                  validTunnelPrefixes(interface: copy.interface.addresses.map { $0.networkPrefixLength },
                                      peer: selected.allowedIPs.map { $0.networkPrefixLength }),
                  (1...25).contains(selected.persistentKeepAlive ?? 0) else { completion(false); return }
            guard let current = self.operations.start(wall: Self.wall(), mono: Self.monotonic()) else { completion(false); return }
            self.peer = selected.publicKey.hexKey
            self.adapter.start(tunnelConfiguration: copy) { error in
                self.queue.async {
                    guard self.operations.started(current, success: error == nil) else { completion(false); return }
                    if error != nil { self.peer = nil }
                    completion(error == nil)
                }
            }
        }
    }

    /// Invalidate status immediately. A late start/status callback cannot restore it.
    public func stop(completion: @escaping (Bool) -> Void) {
        queue.async {
            let current = self.operations.stop()
            self.peer = nil
            self.adapter.stop { error in
                self.queue.async {
                    let stopped: Bool
                    if let error {
                        if case .invalidState = error { stopped = true } else { stopped = false }
                    } else { stopped = true }
                    guard self.operations.stopped(current, success: stopped) else { completion(false); return }
                    completion(stopped)
                }
            }
        }
    }

    /// Returns bounded status only, never WireGuard's raw runtime configuration.
    public func status(completion: @escaping (MeshTunnelStatus) -> Void) {
        queue.async {
            let current = self.operations.stamp
            guard self.operations.phase == .running, let peer = self.peer else {
                completion(self.operations.status(up: false, handshake: 0, wall: Self.wall(), mono: Self.monotonic()))
                return
            }
            self.adapter.getRuntimeConfiguration { settings in
                self.queue.async {
                    guard self.operations.current(current), self.operations.phase == .running else {
                        completion(.unreachable)
                        return
                    }
                    let handshake = settings.flatMap { peerHandshake($0, peer: peer) }
                    completion(self.operations.status(up: handshake != nil, handshake: handshake ?? 0,
                                                   wall: Self.wall(), mono: Self.monotonic()))
                }
            }
        }
    }

    private static func wall() -> Int64 {
        let milliseconds = Date().timeIntervalSince1970 * 1000
        guard milliseconds.isFinite, milliseconds > 0, milliseconds < Double(Int64.max) else { return -1 }
        return Int64(milliseconds)
    }
    private static func monotonic() -> Int64 {
        var scale = mach_timebase_info_data_t()
        guard mach_timebase_info(&scale) == KERN_SUCCESS, scale.denom != 0 else { return -1 }
        let milliseconds = Double(mach_continuous_time()) * Double(scale.numer) / Double(scale.denom) / 1_000_000
        guard milliseconds.isFinite, milliseconds >= 0, milliseconds < Double(Int64.max) else { return -1 }
        return Int64(milliseconds)
    }
}
