// The daemon and permission-bearing desktop implementation share one process.
// No external caller can register a callback or acquire companion authority.
import AppKit
import CoreGraphics

private final class ReplySlot: @unchecked Sendable {
    let lock = NSLock()
    var cancelled = false
    var value: Data?
    func active() -> Bool { lock.lock(); defer { lock.unlock() }; return !cancelled }
    func finish(_ data: Data) { lock.lock(); defer { lock.unlock() }; if !cancelled { value = data } }
    func take() -> Data? { lock.lock(); defer { lock.unlock() }; cancelled = true; return value }
}
private func desktopCallback(_ bytes: UnsafePointer<UInt8>?, _ count: Int, _ output: UnsafeMutablePointer<UnsafeMutablePointer<UInt8>?>?, _ size: UnsafeMutablePointer<Int>?) -> Int32 {
    guard !Thread.isMainThread, let bytes, let output, let size, count > 0, count <= 1024 * 1024 else { return 1 }
    output.pointee = nil; size.pointee = 0
    let request = Data(bytes: bytes, count: count)
    let finished = DispatchSemaphore(value: 0)
    let slot = ReplySlot()
    let task = Task { @MainActor in
        defer { finished.signal() }
        guard slot.active(), !Task.isCancelled else { return }
        var reply: [String: Any]
        do {
            guard let object = try JSONSerialization.jsonObject(with: request) as? [String: Any] else { throw NativeError.invalid }
            reply = ["result": try await NativeRuntime.shared.perform(object)]
        } catch { if slot.active() { releaseHeld() }; reply = ["error": "Native operation refused or unavailable"] }
        if let data = try? JSONSerialization.data(withJSONObject: reply) { slot.finish(data) }
    }
    guard finished.wait(timeout: .now() + 10) == .success else {
        _ = slot.take(); task.cancel()
        Task { @MainActor in NativeRuntime.shared.pause("Native operation timed out") }
        return 1
    }
    guard let data = slot.take(), data.count <= 48 * 1024 * 1024, let memory = malloc(data.count) else { return 1 }
    data.copyBytes(to: memory.assumingMemoryBound(to: UInt8.self), count: data.count)
    output.pointee = memory.assumingMemoryBound(to: UInt8.self); size.pointee = data.count
    return 0
}

@MainActor final class NativeRuntime: NSObject, NSApplicationDelegate {
    static let shared = NativeRuntime()
    private let queue = DispatchQueue(label: "sh.hyperme.visor.internal-control")
    private var timer: Timer?
    private var workspaceObservers: [NSObjectProtocol] = []
    private var statusItem: NSStatusItem?
    private var mascotFrames: [String: [NSImage]] = [:]
    private var mascotTick = 0
    private var usageSummary: String?
    private var engineConnected = false
    private var menuAllow: NSMenuItem?
    private var destinationItems: [NSMenuItem] = []
    private var currentSession: String?
    private var approvedSession: String?
    private var idleResume: NativeIdleResume?
    private var pendingApproval = false
    private var tap: CFMachPort?
    private var tapSource: CFRunLoopSource?
    private var approvedLayout: DesktopLayout?
    private var allowed = false
    private var restartRequired = false
    private var epoch: UInt64 = 0
    private var polling = false
    private var started = false
    private var sessionOpen = false
    private var currentPhase = "off"
    private var message = "Desktop access is off"
    weak var label: NSTextField?
    weak var allowButton: NSButton?
    private(set) var runtimePath = ""

    func start() {
        guard !started else { return }
        let home = FileManager.default.homeDirectoryForCurrentUser.path
        runtimePath = home + "/.local/state/hypermesh/native-run"
        setenv("HYPERMESH_RUNTIME_DIR", runtimePath, 1)
        setenv("HYPERMESH_CONFIG_DIR", home + "/.config/hypermesh", 1)
        // No env/CLI argument can enable this callback; only the app links it.
        guard hypermesh_native_start(desktopCallback) == 0 else { message = "Native engine failed to start"; update(); return }
        started = true
        statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        for state in ["idle", "automation", "disconnected"] {
            let count = state == "disconnected" ? 6 : 8
            mascotFrames[state] = (0..<count).compactMap { frame in
                guard let path = Bundle.main.path(forResource: "\(state)_\(frame)_128", ofType: "png", inDirectory: "companion"),
                      let image = NSImage(contentsOfFile: path) else { return nil }
                image.size = NSSize(width: 22, height: 22)
                image.isTemplate = false
                return image
            }
        }
        let menu = NSMenu(); menu.autoenablesItems = false
        for (title, action) in [("Allow desktop access", #selector(allow)), ("Pause", #selector(pauseButton)), ("Stop", #selector(stopButton))] {
            let item = NSMenuItem(title: title, action: action, keyEquivalent: "")
            item.target = self; menu.addItem(item)
            if action == #selector(allow) { menuAllow = item }
        }
        menu.addItem(.separator())
        for (title, action) in [("Session forum", #selector(openForum)), ("Session in VS Code", #selector(openWorkspace))] {
            let item = NSMenuItem(title: title, action: action, keyEquivalent: "")
            item.target = self; menu.addItem(item); destinationItems.append(item)
        }
        menu.addItem(.separator())
        menu.addItem(NSMenuItem(title: "Quit Visor", action: #selector(NSApplication.terminate(_:)), keyEquivalent: ""))
        statusItem?.menu = menu
        installMonitor()
        for name in [NSWorkspace.willSleepNotification, NSWorkspace.screensDidSleepNotification,
                     NSWorkspace.sessionDidResignActiveNotification] {
            workspaceObservers.append(NSWorkspace.shared.notificationCenter.addObserver(forName: name, object: nil, queue: .main) { [weak self] _ in
                Task { @MainActor in self?.pause("Desktop session became inactive; select Resume to continue") }
            })
        }
        timer = Timer.scheduledTimer(withTimeInterval: 0.25, repeats: true) { _ in Task { @MainActor in self.poll() } }
        update()
    }
    func applicationWillTerminate(_ notification: Notification) {
        allowed = false; idleResume = nil; pendingApproval = false; epoch &+= 1; releaseHeld()
        NativePointerOverlay.shared.hide()
        if let tap { CGEvent.tapEnable(tap: tap, enable: false) }
        timer?.invalidate()
        for observer in workspaceObservers { NSWorkspace.shared.notificationCenter.removeObserver(observer) }
        workspaceObservers.removeAll()
    }
    private func control(_ action: String, session: String? = nil, pauseEpoch: UInt64? = nil, completion: @escaping @MainActor ([String:Any]?) -> Void = { _ in }) {
        queue.async {
            var fields: [String:Any] = ["action": action]
            if let session { fields["session"] = session }
            if let pauseEpoch { fields["epoch"] = pauseEpoch }
            let data = try! JSONSerialization.data(withJSONObject: fields)
            var output: UnsafeMutablePointer<UInt8>?
            var size = 0
            let code = data.withUnsafeBytes { raw in hypermesh_native_control(raw.bindMemory(to: UInt8.self).baseAddress, data.count, &output, &size) }
            var value: [String:Any]?
            if let output {
                defer { free(output) }
                if code == 0 && size >= 0 && size <= 256 * 1024 {
                    value = (try? JSONSerialization.jsonObject(with: Data(bytes: output, count: size))) as? [String:Any]
                }
            }
            let result = value
            Task { @MainActor in completion(result) }
        }
    }
    private func update() {
        NativePointerOverlay.shared.setVisible(nativePointerOverlayVisible(connected: engineConnected,
            sessionOpen: sessionOpen, allowed: allowed, phase: currentPhase, restartRequired: restartRequired))
        let readiness = restartRequired ? "Restore permissions in System Settings, then quit and reopen Visor. Desktop access remains off." : (started && !sessionOpen ? "Waiting for a local client to open a session" : message)
        label?.stringValue = "\(readiness)\nSession: \(currentSession ?? "none") · Engine: \(currentPhase)\nControl socket: \(runtimePath)/control.sock"
        let access = desktopAccessControl(started: started, sessionOpen: sessionOpen, allowed: allowed,
                                          paused: currentPhase == "paused", restartRequired: restartRequired)
        allowButton?.isEnabled = access.enabled && !pendingApproval
        allowButton?.title = access.title
        menuAllow?.isEnabled = access.enabled && !pendingApproval
        menuAllow?.title = access.title
        for item in destinationItems { item.isEnabled = engineConnected && sessionOpen && currentSession != nil }
        let state = nativeMascotState(connected: engineConnected && !restartRequired, allowed: allowed, phase: currentPhase)
        if let frames = mascotFrames[state], frames.count == (state == "disconnected" ? 6 : 8) {
            let frame = NSWorkspace.shared.accessibilityDisplayShouldReduceMotion ? 0 : mascotTick % frames.count
            statusItem?.button?.image = frames[frame]
            statusItem?.button?.title = ""
        } else {
            statusItem?.button?.image = nil
            statusItem?.button?.title = allowed ? "Visor: ON" : "Visor: off"
        }
        statusItem?.button?.setAccessibilityLabel("Hypermesh Visor: " + currentPhase)
        statusItem?.button?.toolTip = [message, "Session: \(sessionOpen ? "open" : "none") · Engine: \(currentPhase)", usageSummary]
            .compactMap { $0 }.joined(separator: "\n")
    }
    private func poll() {
        guard started && !polling else { return }
        mascotTick = (mascotTick + 1) % 24
        polling = true
        let generation = epoch
        control("poll") { result in
            self.polling = false
            guard generation == self.epoch else { return }
            guard let result, result["status"] as? Int == 200, let body = result["body"] as? [String:Any], let automation = body["automation"] as? [String:Any] else {
                self.engineConnected = false
                self.currentSession = nil
                self.usageSummary = nil
                self.pause("Native engine unavailable"); return
            }
            self.engineConnected = true
            self.sessionOpen = body["open"] as? Bool ?? false
            let sessionID = (body["session"] as? [String:Any])?["id"] as? String
            self.currentSession = self.sessionOpen ? sessionID.flatMap { UUID(uuidString: $0)?.uuidString.lowercased() } : nil
            self.usageSummary = self.sessionOpen ? nativeUsageSummary(body["session"] as? [String:Any]) : nil
            self.currentPhase = automation["phase"] as? String ?? "off"
            if self.allowed && (!self.sessionOpen || self.currentPhase != "running" || self.currentSession != self.approvedSession) { self.pause("Session changed, stopped or paused") }
            if self.allowed, let interruption = desktopInterruption(permissionsAndMonitor: self.permissionsAndMonitor(), layoutMatches: self.layoutMatchesApproval()) {
                self.pause(interruption.message, lost: interruption.permanent)
            }
            if let idle = self.idleResume, idle.epoch != nil {
                if !idle.matches(session: self.currentSession, phase: self.currentPhase, epoch: nativeEpoch(automation["epoch"])) {
                    self.pause("Automatic resume cancelled by a newer control or session change")
                } else if let interruption = desktopInterruption(permissionsAndMonitor: self.permissionsAndMonitor(), layoutMatches: self.layoutMatchesApproval()) {
                    self.pause(interruption.message, lost: interruption.permanent)
                } else if idle.ready(now: ProcessInfo.processInfo.systemUptime, inputHeld: self.physicalInputHeld()) {
                    self.resumeAfterIdle(idle)
                }
            }
            self.update()
        }
    }
    @objc func openForum() { openDestination(editor: false) }
    @objc func openWorkspace() { openDestination(editor: true) }
    private func openDestination(editor: Bool) {
        guard engineConnected, sessionOpen, let selected = currentSession else { return }
        control("destinations", session: selected) { result in
            guard self.engineConnected, self.sessionOpen, let destination = nativeDestinationResponse(result, expected: selected,
                    current: self.currentSession, editor: editor) else {
                self.message = "Configure this session's destination in companion.toml, then retry."; self.update(); return
            }
            let completed: @Sendable (NSRunningApplication?, Error?) -> Void = { _, error in
                let failed = error != nil
                Task { @MainActor in
                    if failed && self.currentSession == selected {
                        self.message = "Could not open this session destination."; self.update()
                    }
                }
            }
            let configuration = NSWorkspace.OpenConfiguration()
            if editor {
                var directory: ObjCBool = false
                guard FileManager.default.fileExists(atPath: destination.path, isDirectory: &directory), directory.boolValue,
                      let application = NSWorkspace.shared.urlForApplication(withBundleIdentifier: "com.microsoft.VSCode") else {
                    self.message = "Install Visual Studio Code and configure an existing workspace directory."; self.update(); return
                }
                NSWorkspace.shared.open([destination], withApplicationAt: application, configuration: configuration, completionHandler: completed)
            } else {
                NSWorkspace.shared.open(destination, configuration: configuration, completionHandler: completed)
            }
        }
    }
    @objc func allow() {
        guard started && sessionOpen && !restartRequired && !allowed && !pendingApproval, let selected = currentSession else { return }
        idleResume = nil
        installMonitor()
        let missing = [("Screen Recording", CGPreflightScreenCaptureAccess()),
                       ("Accessibility", CGPreflightPostEventAccess()),
                       ("Input Monitoring", CGPreflightListenEventAccess())].filter { !$0.1 }.map { $0.0 }
        guard missing.isEmpty else { message = "Permission unavailable: " + missing.joined(separator: ", ") + ". Check System Settings, then retry."; update(); return }
        approvedLayout = try? desktopLayout()
        guard permissionsAndMonitor(), layoutMatchesApproval() else { message = "Desktop layout or input monitor unavailable. Check connected displays and restart Visor if needed."; update(); return }
        epoch &+= 1
        let generation = epoch
        pendingApproval = true
        update()
        control(currentPhase == "paused" ? "resume" : "start", session: selected) { result in
            guard generation == self.epoch else { return }
            self.pendingApproval = false
            guard result?["status"] as? Int == 204 else { self.message = "Unable to start; check session and permissions"; self.update(); return }
            guard self.currentSession == selected, self.sessionOpen,
                  self.permissionsAndMonitor(), self.layoutMatchesApproval() else {
                self.pause("Session or desktop access changed during approval"); return
            }
            self.approvedSession = selected
            self.allowed = true; self.message = "Desktop access allowed — real input pauses immediately"; self.currentPhase = "running"; self.update()
        }
    }
    @objc func pauseButton() { pause("Paused by user") }
    @objc func stopButton() { pause("Desktop access stopped"); control("stop") }
    func pause(_ reason: String, lost: Bool = false) {
        allowed = false; approvedSession = nil; idleResume = nil; pendingApproval = false; epoch &+= 1; releaseHeld(); message = reason
        restartRequired = restartRequired || lost
        if started { control(lost ? "monitor-lost" : "pause") }
        update()
    }
    private func physicalInputHeld() -> Bool {
        NSEvent.pressedMouseButtons != 0 ||
            (0..<128).contains { CGEventSource.keyState(.hidSystemState, key: CGKeyCode($0)) } ||
            [CGMouseButton.left, .right, .center].contains { CGEventSource.buttonState(.hidSystemState, button: $0) }
    }
    private func physicalInput() {
        if pendingApproval { pause("Real input cancelled the pending approval"); return }
        let now = ProcessInfo.processInfo.systemUptime
        guard allowed, let selected = approvedSession else { idleResume?.noteInput(now: now); return }
        allowed = false; approvedSession = nil; epoch &+= 1; releaseHeld()
        currentPhase = "paused"
        message = "Paused by real input. Resumes after two idle minutes; Pause or Stop cancels."
        idleResume = NativeIdleResume(session: selected, now: now)
        let generation = epoch
        control("idle-pause", session: selected) { result in
            guard generation == self.epoch, self.idleResume?.session == selected else { return }
            guard result?["status"] as? Int == 200,
                  let pauseEpoch = nativeEpoch((result?["body"] as? [String:Any])?["epoch"]) else {
                self.pause("Paused; select Resume to continue"); return
            }
            self.idleResume?.epoch = pauseEpoch
            self.update()
        }
        update()
    }
    private func resumeAfterIdle(_ idle: NativeIdleResume) {
        guard let pauseEpoch = idle.epoch, !pendingApproval else { return }
        idleResume = nil; pendingApproval = true; epoch &+= 1
        let generation = epoch
        control("idle-resume", session: idle.session, pauseEpoch: pauseEpoch) { result in
            guard generation == self.epoch else { return }
            self.pendingApproval = false
            guard result?["status"] as? Int == 200, self.currentSession == idle.session,
                  self.sessionOpen, !self.restartRequired, self.permissionsAndMonitor(),
                  self.layoutMatchesApproval(), !self.physicalInputHeld() else {
                self.pause("Automatic resume cancelled; select Resume to continue"); return
            }
            self.approvedSession = idle.session; self.allowed = true; self.currentPhase = "running"
            self.message = "Desktop access resumed after two idle minutes — real input pauses immediately"
            self.update()
        }
    }
    private func permissionsAndMonitor() -> Bool {
        guard let tap else { return false }
        return CGPreflightScreenCaptureAccess() && CGPreflightPostEventAccess() && CGPreflightListenEventAccess() && CGEvent.tapIsEnabled(tap: tap)
    }
    private func layoutMatchesApproval() -> Bool {
        guard let approvedLayout, let currentLayout = try? desktopLayout() else { return false }
        return currentLayout == approvedLayout
    }
    private func installMonitor() {
        guard tap == nil, CGPreflightListenEventAccess() else { return }
        let types: [CGEventType] = [.keyDown,.keyUp,.flagsChanged,.mouseMoved,.leftMouseDragged,.rightMouseDragged,.otherMouseDragged,.leftMouseDown,.leftMouseUp,.rightMouseDown,.rightMouseUp,.otherMouseDown,.otherMouseUp,.scrollWheel]
        let mask = types.reduce(CGEventMask(0)) { $0 | (CGEventMask(1) << $1.rawValue) }
        tap = CGEvent.tapCreate(tap: .cgSessionEventTap, place: .headInsertEventTap, options: .listenOnly, eventsOfInterest: mask, callback: { _, type, event, _ in
            // Tap is attached only to the main run loop; callback and input operations serialize there.
            MainActor.assumeIsolated {
                let runtime = NativeRuntime.shared
                if type == .tapDisabledByTimeout || type == .tapDisabledByUserInput {
                    runtime.pause("Input monitor stopped; restart to recover", lost: true)
                } else if !(event.getIntegerValueField(.eventSourceUserData) == injectionTag && event.getIntegerValueField(.eventSourceUnixProcessID) == Int64(getpid())) {
                    runtime.physicalInput()
                }
            }
            return Unmanaged.passUnretained(event)
        }, userInfo: nil)
        if let tap {
            tapSource = CFMachPortCreateRunLoopSource(kCFAllocatorDefault, tap, 0)
            CFRunLoopAddSource(CFRunLoopGetMain(), tapSource, .commonModes)
            CGEvent.tapEnable(tap: tap, enable: true)
        }
    }
    func perform(_ request: [String:Any]) async throws -> [String:Any] {
        guard started, !Task.isCancelled else { throw NativeError.denied }
        if request["action"] as? String == "release" { releaseHeld(); return [:] }
        if request["action"] as? String != "info" {
            guard allowed, permissionsAndMonitor(), layoutMatchesApproval() else { throw NativeError.denied }
        }
        let generation = epoch
        let result = try await handle(request) {
            guard self.allowed, self.epoch == generation, !Task.isCancelled,
                  self.permissionsAndMonitor(), self.layoutMatchesApproval() else { throw NativeError.denied }
        }
        if request["action"] as? String != "info" {
            guard allowed, epoch == generation, !Task.isCancelled else { throw NativeError.denied }
        }
        return result
    }
}
