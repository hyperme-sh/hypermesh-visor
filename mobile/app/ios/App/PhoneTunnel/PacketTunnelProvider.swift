import Foundation
import Darwin
import NetworkExtension
import VisorMeshTunnel
import WireGuardKit

/// The app passes a short lease in memory. Saved VPN preferences never contain keys.
final class PacketTunnelProvider: NEPacketTunnelProvider {
    private lazy var tunnel = AppleMeshTunnel(provider: self)
    private let queue = DispatchQueue(label: "sh.hyperme.visor.phone-provider")
    private var timer: DispatchSourceTimer?
    private var configuration: String?
    private var expires: TimeInterval = 0
    private var deadline: TimeInterval = 0
    private var generation: UInt64 = 0

    private func lease(_ values: [String: Any]) -> (String, TimeInterval)? {
        guard let config = values["configuration"] as? String, config.utf8.count < 4096,
              let expiry = values["expires"] as? NSNumber else { return nil }
        let end = expiry.doubleValue, now = Date().timeIntervalSince1970
        guard end.isFinite, end > now + 2, end <= now + 60 else { return nil }
        return (config, end)
    }
    private func arm(_ end: TimeInterval) {
        expires = end
        deadline = continuousTime() + max(0, end - Date().timeIntervalSince1970)
        timer?.cancel()
        let watchdog = DispatchSource.makeTimerSource(queue: queue)
        watchdog.schedule(deadline: .now(), repeating: 1)
        watchdog.setEventHandler { [weak self] in
            guard let self else { return }
            if !self.current() {
                self.configuration = nil; self.generation &+= 1
                self.timer?.cancel(); self.timer = nil
                let stamp = self.generation
                self.tunnel.stop { _ in
                    self.queue.async {
                        if stamp == self.generation { self.cancelTunnelWithError(nil) }
                    }
                }
            }
        }
        timer = watchdog; watchdog.resume()
    }
    private func current() -> Bool {
        deadline.isFinite && configuration != nil && Date().timeIntervalSince1970 < expires && continuousTime() < deadline
    }
    override func startTunnel(options: [String: NSObject]?, completionHandler: @escaping (Error?) -> Void) {
        queue.async {
            guard self.configuration == nil, let options, let (raw, end) = self.lease(options),
                  let config = phoneConfiguration(raw) else {
                completionHandler(NSError(domain: "VisorTunnel", code: 1)); return
            }
            self.generation &+= 1; let stamp = self.generation
            self.configuration = raw; self.arm(end)
            self.tunnel.start(configuration: config) { started in
                self.queue.async {
                    guard stamp == self.generation else {
                        completionHandler(NSError(domain: "VisorTunnel", code: 1)); return
                    }
                    guard self.current(), started else {
                        self.configuration = nil
                        self.tunnel.stop { _ in completionHandler(NSError(domain: "VisorTunnel", code: 1)) }
                        return
                    }
                    completionHandler(nil)
                }
            }
        }
    }
    override func stopTunnel(with reason: NEProviderStopReason, completionHandler: @escaping () -> Void) {
        queue.async {
            self.generation &+= 1; self.configuration = nil
            self.timer?.cancel(); self.timer = nil
            self.tunnel.stop { _ in completionHandler() }
        }
    }
    override func handleAppMessage(_ data: Data, completionHandler: ((Data?) -> Void)?) {
        queue.async {
            guard data.count < 8192, self.current(),
                  let values = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] else {
                completionHandler?(nil); return
            }
            if values["operation"] as? String == "refresh" {
                guard let (raw, end) = self.lease(values), raw == self.configuration else {
                    completionHandler?(nil); return
                }
                self.arm(end); completionHandler?(Data("ok".utf8))
            } else if values["operation"] as? String == "status" {
                let stamp = self.generation
                self.tunnel.status { status in
                    self.queue.async {
                        guard stamp == self.generation, self.current() else { completionHandler?(nil); return }
                        completionHandler?(Data(status.rawValue.utf8))
                    }
                }
            } else { completionHandler?(nil) }
        }
    }
}

// Accept only the exact minimal configuration emitted by the native lease parser.
private func phoneConfiguration(_ raw: String) -> TunnelConfiguration? {
    var fields: [String: String] = [:]
    var section = ""
    for line in raw.split(separator: "\n") {
        if line == "[Interface]" || line == "[Peer]" { section = String(line); continue }
        let parts = line.split(separator: "=", maxSplits: 1).map { $0.trimmingCharacters(in: .whitespaces) }
        guard parts.count == 2, fields[parts[0]] == nil else { return nil }
        let permitted = section == "[Interface]" ? ["PrivateKey", "Address"] : ["PublicKey", "Endpoint", "AllowedIPs", "PersistentKeepalive"]
        guard permitted.contains(parts[0]) else { return nil }
        fields[parts[0]] = parts[1]
    }
    guard fields.count == 6,
          let privateKey = PrivateKey(base64Key: fields["PrivateKey"] ?? ""),
          let publicKey = PublicKey(base64Key: fields["PublicKey"] ?? ""),
          let address = IPAddressRange(from: fields["Address"] ?? ""), address.networkPrefixLength == 32,
          let allowed = IPAddressRange(from: fields["AllowedIPs"] ?? ""), allowed.networkPrefixLength == 32,
          let endpoint = Endpoint(from: fields["Endpoint"] ?? ""), fields["PersistentKeepalive"] == "15" else { return nil }
    var iface = InterfaceConfiguration(privateKey: privateKey); iface.addresses = [address]
    var peer = PeerConfiguration(publicKey: publicKey)
    peer.allowedIPs = [allowed]; peer.endpoint = endpoint; peer.persistentKeepAlive = 15
    return TunnelConfiguration(name: "Visor", interface: iface, peers: [peer])
}

private func continuousTime() -> TimeInterval {
    var scale = mach_timebase_info_data_t()
    guard mach_timebase_info(&scale) == KERN_SUCCESS, scale.denom != 0 else { return .infinity }
    return Double(mach_continuous_time()) * Double(scale.numer) / Double(scale.denom) / 1_000_000_000
}
