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
    private var trustController: NativeMcpTrustController?
    private var menuAllow: NSMenuItem?
    private var destinationItems: [NSMenuItem] = []
    private var compactUsage: String?
    private let companionSettings = NativeCompanionSettings(defaults: .standard)
    private var floatingItem: NSMenuItem?
    weak var controlWindow: NSWindow?
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
        let directories = nativeStartupDirectories(home: home, environment: ProcessInfo.processInfo.environment)
        runtimePath = directories.runtime
        setenv("HYPERMESH_RUNTIME_DIR", runtimePath, 1)
        setenv("HYPERMESH_CONFIG_DIR", directories.config, 1)
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
        let trust = NSMenuItem(title: "MCP discovery and trust", action: #selector(openMcpTrust), keyEquivalent: "")
        trust.target = self; menu.addItem(trust)
        let controls = NSMenuItem(title: "Open control window", action: #selector(openControls), keyEquivalent: "")
        controls.target = self; menu.insertItem(controls, at: 0)
        let floating = NSMenuItem(title: "Show floating companion", action: #selector(toggleFloating), keyEquivalent: "")
        floating.target = self; menu.insertItem(floating, at: 1); floatingItem = floating
        NativeCompanionPanel.shared.enabled = companionSettings.visible
        NativeCompanionPanel.shared.configure(target: self, action: #selector(openCompanionMenu))
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
        NativeCompanionPanel.shared.hide()
        if let tap { CGEvent.tapEnable(tap: tap, enable: false) }
        timer?.invalidate()
        for observer in workspaceObservers { NSWorkspace.shared.notificationCenter.removeObserver(observer) }
        workspaceObservers.removeAll()
    }
    private func control(_ action: String, session: String? = nil, pauseEpoch: UInt64? = nil, selection: [String:String]? = nil, decision: [String:String]? = nil, completion: @escaping @MainActor ([String:Any]?) -> Void = { _ in }) {
        queue.async {
            var fields: [String:Any] = ["action": action]
            if let session { fields["session"] = session }
            if let pauseEpoch { fields["epoch"] = pauseEpoch }
            if let selection { fields["selection"] = selection }
            if let decision { fields["decision"] = decision }
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
        allowButton?.isEnabled = access.enabled && !pendingApproval && trustController == nil
        allowButton?.title = access.title
        menuAllow?.isEnabled = access.enabled && !pendingApproval && trustController == nil
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
        let detail = [message, "Session: \(sessionOpen ? "open" : "none") · Engine: \(currentPhase)", usageSummary]
            .compactMap { $0 }.joined(separator: "\n")
        statusItem?.button?.toolTip = detail
        floatingItem?.state = NativeCompanionPanel.shared.enabled ? .on : .off
        NativeCompanionPanel.shared.update(image: statusItem?.button?.image,
            state: !engineConnected ? "Disconnected" : (allowed ? "Running" : (currentPhase == "paused" ? "Paused" : "Off")),
            usage: compactUsage, detail: detail, allowed: allowed)

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
                self.compactUsage = nil
                self.pause("Native engine unavailable"); return
            }
            self.engineConnected = true
            self.sessionOpen = body["open"] as? Bool ?? false
            let sessionID = (body["session"] as? [String:Any])?["id"] as? String
            self.currentSession = self.sessionOpen ? sessionID.flatMap { UUID(uuidString: $0)?.uuidString.lowercased() } : nil
            self.usageSummary = self.sessionOpen ? nativeUsageSummary(body["session"] as? [String:Any]) : nil
            self.compactUsage = self.sessionOpen ? nativeCompactUsage(body["session"] as? [String:Any]) : nil
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
    @objc func openControls() {
        controlWindow?.deminiaturize(nil)
        controlWindow?.makeKeyAndOrderFront(nil)
        NSApplication.shared.activate(ignoringOtherApps: true)
    }
    @objc func toggleFloating() {
        NativeCompanionPanel.shared.enabled = companionSettings.toggle()
        update()
    }
    @objc func openCompanionMenu() {
        if let menu = statusItem?.menu { NativeCompanionPanel.shared.showMenu(menu) }
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
    @objc func openMcpTrust() {
        guard started else { return }
        pause("Paused for MCP trust review; close the review before allowing desktop access")
        if trustController == nil {
            trustController = NativeMcpTrustController { [weak self] in self?.trustController = nil; self?.update() }
        }
        trustController?.show()
        update()
    }
    func mcpRequest(_ action: String, selection: [String:String]? = nil, decision: [String:String]? = nil) async throws -> Any {
        let response: [String:Any]? = await withCheckedContinuation { continuation in
            control(action, selection: selection, decision: decision) { continuation.resume(returning: $0) }
        }
        guard let response, let status = response["status"] as? Int, (200..<300).contains(status), let body = response["body"] else { throw NativeError.invalid }
        return body
    }
    @objc func allow() {
        guard trustController == nil && started && sessionOpen && !restartRequired && !allowed && !pendingApproval, let selected = currentSession else { return }
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
        let requestSession = request["session"] as? String
        if request["action"] as? String != "info" {
            guard requestSession != nil, requestSession == approvedSession, allowed, permissionsAndMonitor(), layoutMatchesApproval() else { throw NativeError.denied }
        }
        let generation = epoch
        let result = try await handle(request) {
            guard requestSession != nil, requestSession == self.approvedSession, self.allowed, self.epoch == generation, !Task.isCancelled,
                  self.permissionsAndMonitor(), self.layoutMatchesApproval() else { throw NativeError.denied }
        }
        if request["action"] as? String != "info" {
            guard requestSession != nil, requestSession == approvedSession, allowed, epoch == generation, !Task.isCancelled else { throw NativeError.denied }
        }
        return result
    }
}

@MainActor final class NativeMcpTrustController: NSObject, NSWindowDelegate, NSTextFieldDelegate {
    private let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 860, height: 620), styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: false)
    private let profiles = NSPopUpButton(frame: NSRect(x: 16, y: 572, width: 240, height: 28))
    private let sources = NSPopUpButton(frame: NSRect(x: 270, y: 572, width: 570, height: 28))
    private let details = NSTextView()
    private let message = NSTextField(wrappingLabelWithString: "")
    private let confirmation = NSTextField(frame: NSRect(x: 16, y: 16, width: 110, height: 28))
    private let preview = NSButton(title: "Preview source", target: nil, action: nil)
    private let approve = NSButton(title: "Approve snapshot", target: nil, action: nil)
    private let revoke = NSButton(title: "Revoke profile", target: nil, action: nil)
    private let reload = NSButton(title: "Reload", target: nil, action: nil)
    private let closed: () -> Void
    private var names: [String] = []
    private var sourceIDs: [String?] = []
    private var grants: [Data:String] = [:]
    private var configurationUnavailable = false
    private var pending: String?
    private var busy = false
    private var closing = false
    private var loaded = false

    init(closed: @escaping () -> Void) {
        self.closed = closed
        super.init()
        window.title = "MCP discovery and trust"; window.isReleasedWhenClosed = false
        window.minSize = NSSize(width: 860, height: 620); window.delegate = self; window.center()
        guard let content = window.contentView else { return }
        profiles.target = self; profiles.action = #selector(changed)
        sources.target = self; sources.action = #selector(changed)
        profiles.autoresizingMask = [.minYMargin]; sources.autoresizingMask = [.minYMargin, .width]
        message.frame = NSRect(x: 16, y: 480, width: 824, height: 80); message.autoresizingMask = [.minYMargin, .width]
        let scroll = NSScrollView(frame: NSRect(x: 16, y: 60, width: 824, height: 410))
        scroll.autoresizingMask = [.width, .height]; scroll.hasVerticalScroller = true
        details.isEditable = false; details.isRichText = false; details.isAutomaticLinkDetectionEnabled = false
        details.isVerticallyResizable = true; details.autoresizingMask = [.width]
        details.textContainer?.widthTracksTextView = true
        details.frame = scroll.contentView.bounds
        details.minSize = NSSize(width: 0, height: scroll.contentSize.height)
        details.maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
        scroll.documentView = details
        confirmation.placeholderString = "TRUST / REVOKE"; confirmation.delegate = self
        for (index, pair) in [(preview, #selector(previewClicked)), (approve, #selector(approveClicked)), (revoke, #selector(revokeClicked)), (reload, #selector(reloadClicked))].enumerated() {
            pair.0.frame = NSRect(x: 140 + index * 170, y: 14, width: 160, height: 32)
            pair.0.target = self; pair.0.action = pair.1; content.addSubview(pair.0)
        }
        for view in [profiles, sources, message, scroll, confirmation] as [NSView] { content.addSubview(view) }
        update()
    }
    func show() {
        window.makeKeyAndOrderFront(nil); NSApplication.shared.activate(ignoringOtherApps: true)
        if !loaded { loaded = true; run { try await self.load() } }
    }
    func windowWillClose(_ notification: Notification) { closing = true; invalidate(); closed() }
    func controlTextDidChange(_ notification: Notification) { update() }
    private var profile: String? { names.indices.contains(profiles.indexOfSelectedItem) ? names[profiles.indexOfSelectedItem] : nil }
    private func update() {
        guard !closing else { return }
        profiles.isEnabled = !busy; sources.isEnabled = !busy; reload.isEnabled = !busy; confirmation.isEnabled = !busy
        preview.isEnabled = !busy && profile != nil && sources.indexOfSelectedItem >= 0
        approve.isEnabled = !busy && pending != nil && confirmation.stringValue == "TRUST"
        revoke.isEnabled = !busy && profile.map { grants[Data($0.utf8)] != nil } == true && confirmation.stringValue == "REVOKE"
    }
    private func invalidate() {
        if let id = pending { Task { _ = try? await NativeRuntime.shared.mcpRequest("mcp-decision", decision: ["action": "cancel", "review_id": id]) } }
        pending = nil
        guard !closing else { return }
        confirmation.stringValue = ""; details.string = ""
        message.stringValue = "Current trust: \(nativeMcpSafe(profile.flatMap { grants[Data($0.utf8)] } ?? "not approved")). Preview starts nothing. Approval affects future sessions; close existing sessions to stop their attached servers."
        if configurationUnavailable { message.stringValue += " Profile configuration or sources are unavailable; existing approvals can still be revoked." }
        update()
    }
    @objc private func changed() { invalidate() }
    private func run(_ operation: @escaping @MainActor () async throws -> Void) {
        guard !busy && !closing else { return }
        busy = true; update()
        Task { @MainActor in
            defer { busy = false; update() }
            do { try await operation() }
            catch { if !closing { invalidate(); message.stringValue = "Request unavailable or source changed. Reload and review again. No request was retried." } }
        }
    }
    private func load() async throws {
        invalidate()
        grants = [:]
        guard let trusted = try await NativeRuntime.shared.mcpRequest("mcp-trusted") as? [[String:Any]] else { throw NativeError.invalid }
        let data = (try? await NativeRuntime.shared.mcpRequest("mcp-profiles")) as? [String:Any]
        let sourcesResult = (try? await NativeRuntime.shared.mcpRequest("mcp-sources")) as? [[String:Any]]
        guard !closing else { return }
        let entries = data?["profiles"] as? [[String:Any]] ?? []
        configurationUnavailable = data == nil || sourcesResult == nil
        var trustedNames: [String] = []
        for item in trusted {
            if let name = item["profile"] as? String, let status = item["status"] as? String {
                grants[Data(name.utf8)] = status; trustedNames.append(name)
            }
        }
        names = nativeMcpProfiles(configured: entries.compactMap { $0["name"] as? String }, trusted: trustedNames)
        if names.isEmpty { names = [data?["active"] as? String ?? "default"] }
        profiles.removeAllItems(); profiles.addItems(withTitles: names.enumerated().map { "\($0.offset + 1). \(nativeMcpSafe($0.element))" })
        sources.removeAllItems(); sources.addItems(withTitles: ["Daemon configuration", "Choose an MCP configuration file…"]); sourceIDs = [nil, nil]
        for item in sourcesResult ?? [] where item["status"] as? String == "available_for_review" {
            guard let id = item["id"] as? String else { continue }
            sources.addItem(withTitle: "\(sourceIDs.count + 1). " + nativeMcpSafe("\(item["label"] as? String ?? id) — \(item["path"] as? String ?? "")")); sourceIDs.append(id)
        }
        invalidate()
    }
    @objc private func reloadClicked() { run { try await self.load() } }
    @objc private func previewClicked() {
        run {
            guard let profile = self.profile else { return }
            self.invalidate()
            var selection = ["profile": profile]
            let index = self.sources.indexOfSelectedItem
            if index == 1 {
                let picker = NSOpenPanel(); picker.canChooseDirectories = false; picker.allowsMultipleSelection = false
                guard picker.runModal() == .OK, let url = picker.url, !self.closing else { return }
                selection["project_path"] = url.path
            } else if self.sourceIDs.indices.contains(index), let id = self.sourceIDs[index] { selection["source_id"] = id }
            guard let body = try await NativeRuntime.shared.mcpRequest("mcp-preview", selection: selection) as? [String:Any],
                  let id = nativeMcpReviewID(body, profile: profile) else { throw NativeError.invalid }
            if self.closing { _ = try? await NativeRuntime.shared.mcpRequest("mcp-decision", decision: ["action": "cancel", "review_id": id]); return }
            self.pending = id; self.details.string = nativeMcpDescription(body)
            self.message.stringValue = "Read the source files privately and review the metadata. Type TRUST to approve this exact snapshot for future sessions. Changed or expired snapshots need a fresh review."
        }
    }
    @objc private func approveClicked() {
        run {
            guard let id = self.pending, self.confirmation.stringValue == "TRUST" else { return }
            _ = try await NativeRuntime.shared.mcpRequest("mcp-decision", decision: ["action": "approve", "review_id": id])
            self.pending = nil; try await self.load()
            if !self.closing { self.message.stringValue = "Approved for future sessions. No server was started. Existing attachments are unchanged." }
        }
    }
    @objc private func revokeClicked() {
        run {
            guard let profile = self.profile, self.confirmation.stringValue == "REVOKE" else { return }
            _ = try await NativeRuntime.shared.mcpRequest("mcp-decision", decision: ["action": "revoke", "profile": profile])
            try await self.load()
            if !self.closing { self.message.stringValue = "Revoked for future sessions. Close existing sessions to stop their attached servers." }
        }
    }
}
