import Foundation
import NetworkExtension

private enum TunnelFailure: Error { case unavailable }

/// All methods are native-only. Preferences contain no configuration or credentials.
final class PhoneTunnel {
    let mesh: MobileMeshClient
    private let machine: MobileMachine
    private let owner: MobileOwnerSignIn
    private let ca: String?
    private let lock = NSRecursiveLock()
    private let renewal = DispatchQueue(label: "sh.hyperme.visor.phone-renewal")
    private var timer: DispatchSourceTimer?
    private var manager: NETunnelProviderManager?
    private var lease: MobileMeshLease?
    private var phone: MobilePhoneClient?
    private var generation: UInt64 = 0
    private var wanted = false
    private var configuration: String?
    private var conductor: String?
    private let provider = "sh.hyperme.visor.mobile.tunnel"

    init(machine: MobileMachine, owner: MobileOwnerSignIn, ca: String?) {
        self.machine = machine; self.owner = owner; self.ca = ca; mesh = MobileMeshClient(machine: machine)
        let timer = DispatchSource.makeTimerSource(queue: renewal)
        timer.schedule(deadline: .now() + 20, repeating: 20)
        timer.setEventHandler { [weak self] in self?.refresh() }
        self.timer = timer; timer.resume()
    }
    private func wait(_ action: (@escaping (Error?) -> Void) -> Void) throws {
        let done = DispatchSemaphore(value: 0), resultLock = NSLock()
        var succeeded = false
        action { error in resultLock.lock(); succeeded = error == nil; resultLock.unlock(); done.signal() }
        let timely = done.wait(timeout: .now() + 30) == .success
        resultLock.lock(); let success = succeeded; resultLock.unlock()
        guard timely && success else { throw TunnelFailure.unavailable }
    }
    private func message(_ session: NETunnelProviderSession, _ values: [String: Any]) throws -> Data {
        let data = try JSONSerialization.data(withJSONObject: values)
        let done = DispatchSemaphore(value: 0), resultLock = NSLock()
        var result: Data?
        try session.sendProviderMessage(data) { reply in resultLock.lock(); result = reply; resultLock.unlock(); done.signal() }
        let timely = done.wait(timeout: .now() + 3) == .success
        resultLock.lock(); let reply = result; resultLock.unlock()
        guard timely, let reply else { throw TunnelFailure.unavailable }
        return reply
    }
    func connect() throws {
        disconnect()
        lock.lock(); wanted = true; let stamp = generation; lock.unlock()
        do { try prepare(stamp) } catch { fail(stamp); throw error }
    }
    private func prepare(_ stamp: UInt64) throws {
        let next = try mesh.prepare(owner: owner)
        let raw = try next.wireguardConfiguration(), origin = try next.conductorOrigin()
        lock.lock(); let existing = manager; lock.unlock()
        let selected: NETunnelProviderManager
        if let existing { selected = existing }
        else {
            let resultLock = NSLock(); var found: NETunnelProviderManager?
            try wait { completion in
                NETunnelProviderManager.loadAllFromPreferences { managers, error in
                    resultLock.lock()
                    found = managers?.first { ($0.protocolConfiguration as? NETunnelProviderProtocol)?.providerBundleIdentifier == self.provider }
                    resultLock.unlock(); completion(error)
                }
            }
            resultLock.lock(); let saved = found; resultLock.unlock()
            selected = saved ?? NETunnelProviderManager()
            let proto = NETunnelProviderProtocol()
            proto.providerBundleIdentifier = provider; proto.serverAddress = "Visor Mesh"
            proto.providerConfiguration = [:]; proto.disconnectOnSleep = true
            selected.protocolConfiguration = proto; selected.localizedDescription = "Visor Mesh"
            selected.isEnabled = true; selected.isOnDemandEnabled = false; selected.onDemandRules = []
            try wait { selected.saveToPreferences(completionHandler: $0) }
            try wait { selected.loadFromPreferences(completionHandler: $0) }
        }
        lock.lock(); defer { lock.unlock() }
        guard wanted, generation == stamp, next.valid(), let session = selected.connection as? NETunnelProviderSession else { throw TunnelFailure.unavailable }
        let nextPhone: MobilePhoneClient?
        if phone == nil || conductor != origin {
            nextPhone = try MobilePhoneClient(machine: machine, conductorOrigin: origin, conductorCaPem: ca)
        } else { nextPhone = phone }
        let wasConfigured = manager != nil
        manager = selected
        let values: [String: Any] = ["operation": "refresh", "configuration": raw, "expires": NSNumber(value: next.expiresAt())]
        if wasConfigured && raw == configuration && session.status == .connected {
            guard try message(session, values) == Data("ok".utf8) else { throw TunnelFailure.unavailable }
        } else {
            session.stopTunnel()
            let timeout = Date().addingTimeInterval(5)
            while session.status != .disconnected && session.status != .invalid && Date() < timeout {
                Thread.sleep(forTimeInterval: 0.05)
            }
            guard session.status == .disconnected || session.status == .invalid else { throw TunnelFailure.unavailable }
            try session.startTunnel(options: ["configuration": raw as NSString, "expires": NSNumber(value: next.expiresAt())])
        }
        phone = nextPhone; lease = next; configuration = raw; conductor = origin
    }
    private func refresh() {
        lock.lock(); let active = wanted && lease != nil; let stamp = generation; lock.unlock()
        guard active else { return }
        do { try prepare(stamp) } catch { fail(stamp) }
    }
    private func fail(_ stamp: UInt64) { lock.lock(); defer { lock.unlock() }; if stamp == generation { disconnect() } }
    func disconnect() {
        lock.lock(); defer { lock.unlock() }
        generation &+= 1; wanted = false; mesh.cancel(); owner.cancelPhoneOperations()
        manager?.connection.stopVPNTunnel(); lease = nil; phone = nil; configuration = nil; conductor = nil
    }
    func status() -> String {
        lock.lock(); defer { lock.unlock() }
        guard wanted else { return "disconnected" }
        guard let lease, lease.valid() else { disconnect(); return "unreachable" }
        guard let session = manager?.connection as? NETunnelProviderSession else { return "unreachable" }
        if session.status == .connecting || session.status == .reasserting { return "connecting" }
        guard session.status == .connected else { return "unreachable" }
        guard let data = try? message(session, ["operation": "status"]),
              let result = String(data: data, encoding: .utf8),
              ["connected", "connecting", "reconnecting", "unreachable"].contains(result) else { return "unreachable" }
        return result
    }
    func client() throws -> MobilePhoneClient {
        lock.lock(); defer { lock.unlock() }
        guard status() == "connected", let phone else { throw TunnelFailure.unavailable }
        return phone
    }
    deinit { timer?.cancel(); disconnect() }
}
