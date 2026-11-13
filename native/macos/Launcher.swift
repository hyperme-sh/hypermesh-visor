// Standalone package launcher. Never link this program into the desktop app.
import AppKit
import Darwin
import Foundation

private let appName = "Hypermesh Visor Development.app"
private let appID = "sh.hyperme.visor.development"
private enum LaunchError: Error { case invalidConfiguration, alreadyRunning, unavailable }

struct LaunchPlan {
    let config: String
    let journal: String
    let socket: String
    let appEnvironment: [String: String]
    let childEnvironment: [String: String]

    init(environment: [String: String], socket: String) throws {
        func absolute(_ key: String) throws -> String {
            guard let value = environment[key], value.hasPrefix("/"),
                  !value.utf8.contains(0), !value.split(separator: "/").contains("..") else {
                throw LaunchError.invalidConfiguration
            }
            return value
        }
        config = try absolute("HYPERMESH_CONDUCTOR_CONFIG")
        journal = try absolute("HYPERMESH_CONDUCTOR_JOURNAL")
        guard socket.hasPrefix("/"), socket.utf8.count < 104 else { throw LaunchError.invalidConfiguration }
        self.socket = socket
        // Explicit allowlist: service credentials and provider environment stay in Conductor.
        let appKeys = Set(["HOME", "USER", "LOGNAME", "PATH", "TMPDIR", "LANG", "LC_ALL",
                           "HYPERMESH_DEFAULT_MODEL", "HYPERMESH_CONDUCTOR_LEASE_ID"])
        var app = environment.filter { appKeys.contains($0.key) }
        app["HYPERMESH_CONDUCTOR_SOCKET"] = socket
        appEnvironment = app
        var child = environment.filter { !$0.key.hasPrefix("DYLD_") && !$0.key.hasPrefix("LD_") }
        child["HYPERMESH_CONDUCTOR_SOCKET"] = socket
        childEnvironment = child
    }

    func arguments(parent: Int32) -> [String] {
        ["serve", "--socket", "--parent-pid", String(parent), "--config", config, "--journal", journal]
    }
}

enum LaunchAction: Equatable { case wait, openApp, stop(Int32) }
func nextAction(childAlive: Bool, ready: Bool, appOpened: Bool, appAlive: Bool,
                expired: Bool, stopping: Bool) -> LaunchAction {
    if stopping { return .stop(0) }
    if !childAlive { return .stop(1) }
    if appOpened { return appAlive ? .wait : .stop(0) }
    if expired { return .stop(1) }
    return ready ? .openApp : .wait
}

private func privatePath(_ path: String, type: mode_t) -> Bool {
    var info = stat()
    return lstat(path, &info) == 0 && info.st_uid == geteuid()
        && info.st_mode & mode_t(S_IFMT) == type && info.st_mode & 0o077 == 0
}

private struct SocketIdentity: Equatable {
    let device: dev_t
    let inode: ino_t
    static func read(_ path: String) -> SocketIdentity? {
        var info = stat()
        guard lstat(path, &info) == 0, info.st_uid == geteuid(),
              info.st_mode & mode_t(S_IFMT) == mode_t(S_IFSOCK), info.st_mode & 0o077 == 0 else { return nil }
        return SocketIdentity(device: info.st_dev, inode: info.st_ino)
    }
    func removeIfUnchanged(_ path: String) {
        if Self.read(path) == self { _ = unlink(path) }
    }
}

// Bounded readiness probe; it carries no credentials or model request.
private func ready(_ path: String, childPID: Int32) -> Bool {
    guard privatePath(URL(fileURLWithPath: path).deletingLastPathComponent().path, type: mode_t(S_IFDIR)),
          privatePath(path, type: mode_t(S_IFSOCK)) else { return false }
    let fd = Darwin.socket(AF_UNIX, SOCK_STREAM, 0)
    guard fd >= 0 else { return false }
    defer { close(fd) }
    var timeout = timeval(tv_sec: 0, tv_usec: 200_000)
    var one: Int32 = 1
    guard setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &timeout, socklen_t(MemoryLayout.size(ofValue: timeout))) == 0,
          setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &timeout, socklen_t(MemoryLayout.size(ofValue: timeout))) == 0,
          setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &one, socklen_t(MemoryLayout.size(ofValue: one))) == 0 else { return false }
    var address = sockaddr_un()
    address.sun_family = sa_family_t(AF_UNIX)
    address.sun_len = UInt8(MemoryLayout<sockaddr_un>.size)
    let bytes = Array(path.utf8) + [0]
    guard bytes.count <= MemoryLayout.size(ofValue: address.sun_path) else { return false }
    withUnsafeMutableBytes(of: &address.sun_path) { target in target.copyBytes(from: bytes) }
    let connected = withUnsafePointer(to: &address) {
        $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
            Darwin.connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
        }
    }
    guard connected == 0 else { return false }
    var uid: uid_t = 0
    var gid: gid_t = 0
    var pid: pid_t = 0
    var length = socklen_t(MemoryLayout.size(ofValue: pid))
    guard getpeereid(fd, &uid, &gid) == 0, uid == geteuid(),
          getsockopt(fd, SOL_LOCAL, LOCAL_PEERPID, &pid, &length) == 0, pid == childPID else { return false }
    let request = Array("GET /v1/models HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n".utf8)
    guard request.withUnsafeBytes({ Darwin.write(fd, $0.baseAddress, $0.count) }) == request.count else { return false }
    var response = [UInt8](repeating: 0, count: 64)
    let count = Darwin.read(fd, &response, response.count)
    return count >= 13 && String(decoding: response.prefix(count), as: UTF8.self).hasPrefix("HTTP/1.1 200 ")
}

private final class Launcher {
    let child = Process()
    var app: NSRunningApplication?
    var signals: [DispatchSourceSignal] = []
    var timer: Timer?
    var opening = false
    var stopping = false
    var socketDirectory = ""
    var socketIdentity: SocketIdentity?
    var lockFD: Int32 = -1
    var stopCode: Int32 = 0
    var stopDeadline: Date?
    var childKilled = false
    var appKilled = false
    var startupDeadline = Date()
    var plan: LaunchPlan!
    var appURL: URL!

    func start() throws {
        guard geteuid() != 0 else { throw LaunchError.invalidConfiguration }
        let environment = ProcessInfo.processInfo.environment
        // Validate inputs before creating paths or starting a process.
        _ = try LaunchPlan(environment: environment, socket: "/tmp/validate.sock")
        let root = URL(fileURLWithPath: CommandLine.arguments[0]).standardizedFileURL
            .deletingLastPathComponent().deletingLastPathComponent()
        appURL = root.appendingPathComponent(appName)
        guard FileManager.default.isExecutableFile(atPath: root.appendingPathComponent("libexec/conductor").path),
              Bundle(url: appURL)?.bundleIdentifier == appID else { throw LaunchError.unavailable }
        let state = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".local/state/hypermesh/native-launcher")
        try FileManager.default.createDirectory(at: state, withIntermediateDirectories: true,
                                               attributes: [.posixPermissions: 0o700])
        guard privatePath(state.path, type: mode_t(S_IFDIR)) else { throw LaunchError.unavailable }
        lockFD = Darwin.open(state.appendingPathComponent("lock").path, O_RDWR | O_CREAT | O_NOFOLLOW | O_CLOEXEC, 0o600)
        guard lockFD >= 0 else { throw LaunchError.unavailable }
        var info = stat()
        guard fstat(lockFD, &info) == 0, info.st_uid == geteuid(),
              info.st_mode & mode_t(S_IFMT) == mode_t(S_IFREG), info.st_mode & 0o077 == 0,
              flock(lockFD, LOCK_EX | LOCK_NB) == 0,
              NSRunningApplication.runningApplications(withBundleIdentifier: appID).isEmpty else {
            throw LaunchError.alreadyRunning
        }
        // A short unique directory avoids Darwin's small Unix-address limit.
        var template = Array("/tmp/hypermesh-visor.XXXXXX".utf8CString)
        guard let directory = mkdtemp(&template) else { throw LaunchError.unavailable }
        socketDirectory = String(cString: directory)
        plan = try LaunchPlan(environment: environment, socket: socketDirectory + "/conductor.sock")
        child.executableURL = root.appendingPathComponent("libexec/conductor")
        child.arguments = plan.arguments(parent: getpid())
        child.environment = plan.childEnvironment
        child.standardInput = FileHandle.nullDevice
        // Avoid leaking config, provider responses or service credentials in launcher output.
        child.standardOutput = FileHandle.nullDevice
        child.standardError = FileHandle.nullDevice
        for number in [SIGINT, SIGTERM, SIGHUP] {
            signal(number, SIG_IGN)
            let source = DispatchSource.makeSignalSource(signal: number, queue: .main)
            source.setEventHandler { [weak self] in self?.stop(0) }
            source.resume()
            signals.append(source)
        }
        try child.run()
        startupDeadline = Date().addingTimeInterval(10)
        timer = Timer.scheduledTimer(withTimeInterval: 0.1, repeats: true) { [weak self] _ in self?.tick() }
    }

    func tick() {
        if stopping {
            if let deadline = stopDeadline, Date() >= deadline {
                if child.isRunning && !childKilled { childKilled = true; kill(child.processIdentifier, SIGKILL) }
                if let app, !app.isTerminated && !appKilled { appKilled = true; _ = app.forceTerminate() }
            }
            // An in-flight launch must deliver its handle before cleanup can finish.
            if !child.isRunning && (app == nil || app!.isTerminated) && !opening { finish(stopCode) }
            return
        }
        if opening {
            if !child.isRunning || Date() >= startupDeadline { stop(1) }
            return
        }
        let socketReady = app == nil && ready(plan.socket, childPID: child.processIdentifier)
        if socketReady { socketIdentity = SocketIdentity.read(plan.socket) }
        switch nextAction(childAlive: child.isRunning, ready: socketReady,
                          appOpened: app != nil, appAlive: app?.isTerminated == false,
                          expired: Date() >= startupDeadline, stopping: false) {
        case .wait: break
        case .stop(let code): stop(code)
        case .openApp:
            opening = true
            let configuration = NSWorkspace.OpenConfiguration()
            configuration.createsNewApplicationInstance = true
            configuration.allowsRunningApplicationSubstitution = false
            configuration.environment = plan.appEnvironment
            NSWorkspace.shared.openApplication(at: appURL, configuration: configuration) { [weak self] app, error in
                DispatchQueue.main.async {
                    guard let self else { return }
                    self.opening = false
                    self.app = app
                    guard error == nil, let app,
                          app.bundleURL?.standardizedFileURL == self.appURL.standardizedFileURL else {
                        self.stop(1); return
                    }
                    if self.stopping { _ = app.terminate() }
                }
            }
        }
    }

    func stop(_ code: Int32) {
        if stopping { return }
        stopping = true
        stopCode = code
        if code != 0 { fputs("Visor startup or Conductor failed; no provider fallback was used.\n", stderr) }
        stopDeadline = Date().addingTimeInterval(3)
        if child.isRunning { child.terminate() }
        if let app, !app.isTerminated { _ = app.terminate() }
    }

    func finish(_ code: Int32) -> Never {
        // Also remove a crash-left socket only if it is the authenticated instance.
        // Never recursively delete a replaced path or unrelated directory content.
        if let plan { socketIdentity?.removeIfUnchanged(plan.socket) }
        if !socketDirectory.isEmpty { _ = rmdir(socketDirectory) }
        if lockFD >= 0 { close(lockFD) }
        exit(code)
    }
}

private func socketContracts() throws {
    var template = Array("/tmp/visor-launch-test.XXXXXX".utf8CString)
    guard let directory = mkdtemp(&template) else { throw LaunchError.unavailable }
    let root = String(cString: directory)
    let path = root + "/c.sock"
    defer { unlink(path); rmdir(root) }
    let fd = Darwin.socket(AF_UNIX, SOCK_STREAM, 0)
    guard fd >= 0 else { throw LaunchError.unavailable }
    defer { close(fd) }
    var address = sockaddr_un()
    address.sun_family = sa_family_t(AF_UNIX)
    address.sun_len = UInt8(MemoryLayout<sockaddr_un>.size)
    withUnsafeMutableBytes(of: &address.sun_path) { $0.copyBytes(from: Array(path.utf8) + [0]) }
    let bound = withUnsafePointer(to: &address) {
        $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
            Darwin.bind(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
        }
    }
    precondition(bound == 0 && chmod(path, 0o600) == 0 && listen(fd, 4) == 0)
    precondition(!ready(path, childPID: getpid() + 1))
    // Drain the refused peer check; no HTTP bytes may have been sent.
    let refused = accept(fd, nil, nil)
    precondition(refused >= 0)
    var byte: UInt8 = 0
    precondition(read(refused, &byte, 1) == 0)
    close(refused)
    precondition(chmod(path, 0o666) == 0)
    precondition(!ready(path, childPID: getpid()))
    precondition(chmod(path, 0o600) == 0)
    let done = DispatchSemaphore(value: 0)
    DispatchQueue.global().async {
        let peer = accept(fd, nil, nil)
        precondition(peer >= 0)
        defer { close(peer); done.signal() }
        var request = [UInt8](repeating: 0, count: 256)
        let count = read(peer, &request, request.count)
        precondition(count > 0)
        let text = String(decoding: request.prefix(count), as: UTF8.self)
        precondition(text == "GET /v1/models HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        let response = Array("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}".utf8)
        precondition(response.withUnsafeBytes { write(peer, $0.baseAddress, $0.count) } == response.count)
    }
    precondition(ready(path, childPID: getpid()))
    precondition(done.wait(timeout: .now() + 2) == .success)
    let identity = SocketIdentity.read(path)!
    // A replacement pathname must survive cleanup of the original socket.
    let renamed = root + "/original.sock"
    precondition(rename(path, renamed) == 0)
    defer { unlink(renamed) }
    precondition(FileManager.default.createFile(atPath: path, contents: Data("replacement".utf8)))
    identity.removeIfUnchanged(path)
    precondition(FileManager.default.fileExists(atPath: path))
    precondition(unlink(path) == 0 && rename(renamed, path) == 0)
    identity.removeIfUnchanged(path)
    precondition(!FileManager.default.fileExists(atPath: path))
}

private func contracts() throws {
    try socketContracts()
    let environment = ["HOME": "/Users/test", "HYPERMESH_CONDUCTOR_CONFIG": "/config with ' quotes",
                       "HYPERMESH_CONDUCTOR_JOURNAL": "/journal", "CONDUCTOR_AIUDITOR_TOKEN": "secret",
                       "HYPERMESH_DEFAULT_MODEL": "model", "HYPERMESH_CONDUCTOR_LEASE_ID": "lease",
                       "HYPERMESH_CONDUCTOR_ADDR": "old", "HYPERMESH_SUPERVISOR_URL": "old",
                       "DYLD_INSERT_LIBRARIES": "/bad"]
    let plan = try LaunchPlan(environment: environment, socket: "/tmp/private/c.sock")
    precondition(plan.arguments(parent: 123) == ["serve", "--socket", "--parent-pid", "123", "--config", "/config with ' quotes", "--journal", "/journal"])
    precondition(plan.appEnvironment["CONDUCTOR_AIUDITOR_TOKEN"] == nil)
    precondition(plan.appEnvironment["HYPERMESH_CONDUCTOR_CONFIG"] == nil)
    precondition(plan.appEnvironment["HYPERMESH_CONDUCTOR_ADDR"] == nil)
    precondition(plan.appEnvironment["HYPERMESH_SUPERVISOR_URL"] == nil)
    precondition(plan.appEnvironment["HYPERMESH_DEFAULT_MODEL"] == "model")
    precondition(plan.appEnvironment["HYPERMESH_CONDUCTOR_LEASE_ID"] == "lease")
    precondition(plan.childEnvironment["CONDUCTOR_AIUDITOR_TOKEN"] == "secret")
    precondition(plan.childEnvironment["DYLD_INSERT_LIBRARIES"] == nil)
    for key in ["HYPERMESH_CONDUCTOR_CONFIG", "HYPERMESH_CONDUCTOR_JOURNAL"] {
        for value in [nil, "relative", "/tmp/../bad", "/bad\0path"] as [String?] {
            var invalid = environment
            invalid[key] = value
            do { _ = try LaunchPlan(environment: invalid, socket: "/tmp/c.sock"); preconditionFailure("accepted invalid path") }
            catch LaunchError.invalidConfiguration {}
        }
    }
    precondition(nextAction(childAlive: true, ready: false, appOpened: false, appAlive: false, expired: false, stopping: false) == .wait)
    precondition(nextAction(childAlive: true, ready: true, appOpened: false, appAlive: false, expired: false, stopping: false) == .openApp)
    precondition(nextAction(childAlive: false, ready: true, appOpened: true, appAlive: true, expired: false, stopping: false) == .stop(1))
    precondition(nextAction(childAlive: true, ready: true, appOpened: true, appAlive: false, expired: false, stopping: false) == .stop(0))
    precondition(nextAction(childAlive: true, ready: false, appOpened: false, appAlive: false, expired: true, stopping: false) == .stop(1))
    precondition(nextAction(childAlive: true, ready: true, appOpened: true, appAlive: true, expired: true, stopping: false) == .wait)
    print("Native launcher unit contracts passed")
}

if CommandLine.arguments.dropFirst() == ["--self-test"] {
    do { try contracts() } catch { exit(1) }
} else if CommandLine.arguments.dropFirst() == ["--help"] {
    print("Set HYPERMESH_CONDUCTOR_CONFIG and HYPERMESH_CONDUCTOR_JOURNAL to absolute paths, then run bin/hypermesh-visor. Quit Visor to stop its Conductor.")
} else if CommandLine.arguments.count != 1 {
    fputs("Use --help or run without arguments.\n", stderr)
    exit(2)
} else {
    let launcher = Launcher()
    do { try launcher.start() } catch {
        fputs("Unable to start Visor: check configuration paths and close any existing Visor instance.\n", stderr)
        launcher.finish(1)
    }
    withExtendedLifetime(launcher) { RunLoop.main.run() }
}
