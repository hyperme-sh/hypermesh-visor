import AppKit
import CoreGraphics
import ScreenCaptureKit

let injectionTag: Int64 = 0x48595045524d4553
let source = CGEventSource(stateID: .privateState)!
var heldKeys = Set<CGKeyCode>()
var heldButtons = Set<UInt32>()
// Posted movement is asynchronous; subsequent buttons use the sequence target.
var sequencePoint: CGPoint?

func post(_ event: CGEvent?) throws {
    guard CGPreflightPostEventAccess(), let event else { throw NativeError.denied }
    event.setIntegerValueField(.eventSourceUserData, value: injectionTag)
    event.post(tap: .cghidEventTap)
}
enum NativeError: Error { case denied, invalid, unavailable }
func integer(_ value: Any?) throws -> Int {
    guard let number = value as? NSNumber else { throw NativeError.invalid }
    return number.intValue
}
struct DesktopInterruption {
    let message: String
    let permanent: Bool
}
func desktopInterruption(permissionsAndMonitor: Bool, layoutMatches: Bool) -> DesktopInterruption? {
    if !permissionsAndMonitor { return DesktopInterruption(message: "Permission or input monitor lost", permanent: true) }
    if !layoutMatches { return DesktopInterruption(message: "Display layout changed; select Allow to use the new layout", permanent: false) }
    return nil
}

struct DisplayGeometry: Equatable {
    let id: CGDirectDisplayID
    let bounds: CGRect
}
struct DesktopLayout: Equatable {
    let displays: [DisplayGeometry]
    let bounds: CGRect
    init(_ displays: [DisplayGeometry]) throws {
        guard !displays.isEmpty, displays.count <= 16,
              Set(displays.map { $0.id }).count == displays.count else { throw NativeError.unavailable }
        var union = CGRect.null
        for display in displays {
            let b = display.bounds
            guard [b.minX, b.minY, b.width, b.height].allSatisfy({ $0.isFinite && $0.rounded() == $0 }),
                  b.width > 0, b.height > 0 else { throw NativeError.unavailable }
            union = union.union(b)
        }
        guard union.width <= 16384, union.height <= 16384,
              union.width * union.height <= 16_777_216 else { throw NativeError.unavailable }
        self.displays = displays.sorted { $0.id < $1.id }; bounds = union
    }
    func point(x: Int, y: Int) throws -> CGPoint {
        guard x >= 0, y >= 0, Double(x) < bounds.width, Double(y) < bounds.height else { throw NativeError.invalid }
        let point = CGPoint(x: bounds.minX + Double(x), y: bounds.minY + Double(y))
        guard displays.contains(where: { $0.bounds.contains(point) }) else { throw NativeError.invalid }
        return point
    }
    // CoreGraphics drawing uses upward y; desktop coordinates use downward y.
    func imageRect(_ display: DisplayGeometry) -> CGRect {
        CGRect(x: display.bounds.minX - bounds.minX, y: bounds.maxY - display.bounds.maxY,
               width: display.bounds.width, height: display.bounds.height)
    }
}
func desktopLayout() throws -> DesktopLayout {
    var count: UInt32 = 0
    guard CGGetActiveDisplayList(0, nil, &count) == .success, count > 0, count <= 16 else { throw NativeError.unavailable }
    var ids = [CGDirectDisplayID](repeating: 0, count: Int(count))
    guard CGGetActiveDisplayList(count, &ids, &count) == .success, Int(count) <= ids.count else { throw NativeError.unavailable }
    return try DesktopLayout(ids.prefix(Int(count)).map { DisplayGeometry(id: $0, bounds: CGDisplayBounds($0)) })
}

func moveEvent(point: CGPoint, buttons: Set<UInt32>) -> CGEvent? {
    let button = [UInt32(0), 1, 2].first(where: { buttons.contains($0) })
    let type: CGEventType = button == 0 ? .leftMouseDragged : button == 1 ? .rightMouseDragged : button == 2 ? .otherMouseDragged : .mouseMoved
    return CGEvent(mouseEventSource: source, mouseType: type, mouseCursorPosition: point,
                   mouseButton: CGMouseButton(rawValue: button ?? 0)!)
}
func pointer(_ step: [String: Any], layout: DesktopLayout) throws {
    let point = sequencePoint ?? CGEvent(source: nil)?.location ?? .zero
    switch step["kind"] as? String {
    case "move":
        let x = try integer(step["x"]), y = try integer(step["y"])
        let target = try layout.point(x: x, y: y)
        try post(moveEvent(point: target, buttons: heldButtons))
        sequencePoint = target
    case "button":
        let button = try integer(step["button"])
        guard (0...2).contains(button), let down = step["down"] as? Bool else { throw NativeError.invalid }
        let types: [(CGEventType, CGEventType)] = [(.leftMouseDown,.leftMouseUp),(.rightMouseDown,.rightMouseUp),(.otherMouseDown,.otherMouseUp)]
        try post(CGEvent(mouseEventSource: source, mouseType: down ? types[button].0 : types[button].1,
                         mouseCursorPosition: point, mouseButton: CGMouseButton(rawValue: UInt32(button))!))
        if down { heldButtons.insert(UInt32(button)) } else { heldButtons.remove(UInt32(button)) }
    case "scroll":
        let direction = step["direction"] as? String
        guard ["up","down","left","right"].contains(direction ?? "") else { throw NativeError.invalid }
        let dy: Int32 = direction == "up" ? 1 : direction == "down" ? -1 : 0
        let dx: Int32 = direction == "left" ? 1 : direction == "right" ? -1 : 0
        try post(CGEvent(scrollWheelEvent2Source: source, units: .line, wheelCount: 2, wheel1: dy, wheel2: dx, wheel3: 0))
    default: throw NativeError.invalid
    }
}
let namedKeys: [UInt32: CGKeyCode] = [0xff0d:36,0xff09:48,0xff08:51,0xff1b:53,0xff51:123,0xff52:126,0xff53:124,0xff54:125,
    0xffff:117,0xff50:115,0xff57:119,0xff55:116,0xff56:121,0xffe1:56,0xffe3:59,0xffe9:58,0xffeb:55,
    0xffbe:122,0xffbf:120,0xffc0:99,0xffc1:118,0xffc2:96,0xffc3:97,0xffc4:98,0xffc5:100,0xffc6:101,0xffc7:109,0xffc8:103,0xffc9:111]
let characterCodes: [Character: CGKeyCode] = ["a":0,"s":1,"d":2,"f":3,"h":4,"g":5,"z":6,"x":7,"c":8,"v":9,"b":11,"q":12,"w":13,"e":14,"r":15,"y":16,"t":17,"o":31,"u":32,"i":34,"p":35,"l":37,"j":38,"k":40,"n":45,"m":46]
let nonletterCodes: [Character: CGKeyCode] = ["1":18,"2":19,"3":20,"4":21,"6":22,"5":23,"=":24,"9":25,"7":26,"-":27,"8":28,"0":29,
    "]":30,"[":33,"'":39,";":41,"\\":42,",":43,"/":44,".":47,"`":50," ":49]
let shiftedCharacters: [Character: Character] = ["!":"1","@":"2","#":"3","$":"4","%":"5","^":"6","&":"7","*":"8","(":"9",
    ")":"0","_":"-","+":"=","{":"[","}":"]",":":";","\"":"'","|":"\\","<":",",">":".","?":"/","~":"`"]
func physicalKey(_ text: String) throws -> (CGKeyCode, Bool) {
    guard text.utf8.count == 1, let character = text.first else { throw NativeError.invalid }
    if let plain = shiftedCharacters[character], let code = nonletterCodes[plain] { return (code, true) }
    if let code = nonletterCodes[character] { return (code, false) }
    if let lower = text.lowercased().first, let code = characterCodes[lower] {
        return (code, text != text.lowercased())
    }
    throw NativeError.invalid
}
func keyEvent(_ stroke: [String: Any], held: Set<CGKeyCode>) throws -> (CGEvent, Set<CGKeyCode>) {
    let raw = try integer(stroke["keysym"])
    guard raw >= 0, raw <= 0x10ffffff, let down = stroke["down"] as? Bool else { throw NativeError.invalid }
    let keysym = UInt32(raw)
    let shortcut = !held.isDisjoint(with: [55,56,58,59])
    var code: CGKeyCode
    var text: String? = nil
    var implicitShift = false
    if let named = namedKeys[keysym] { code = named }
    else {
        let scalarValue = keysym >= 0x01000000 ? keysym & 0x00ffffff : keysym
        guard let scalar = UnicodeScalar(scalarValue), (keysym < 0xff00 || keysym >= 0x01000000) else { throw NativeError.invalid }
        let character = String(scalar)
        if shortcut {
            // Physical shortcuts use explicit US ANSI key identities, never a Unicode fallback.
            (code, implicitShift) = try physicalKey(character)
        } else {
            text = character
            code = (try? physicalKey(character).0) ?? 0
        }
    }
    guard let event = CGEvent(keyboardEventSource: source, virtualKey: code, keyDown: down) else { throw NativeError.unavailable }
    let next = down ? held.union([code]) : held.subtracting([code])
    var flags: CGEventFlags = []
    if next.contains(56) || implicitShift { flags.insert(.maskShift) }
    if next.contains(59) { flags.insert(.maskControl) }
    if next.contains(58) { flags.insert(.maskAlternate) }
    if next.contains(55) { flags.insert(.maskCommand) }
    event.flags = flags
    if let text {
        let utf16 = Array(text.utf16)
        event.keyboardSetUnicodeString(stringLength: utf16.count, unicodeString: utf16)
    }
    return (event, next)
}
func key(_ stroke: [String: Any]) throws {
    let (event, next) = try keyEvent(stroke, held: heldKeys)
    try post(event)
    heldKeys = next
}
func releaseHeld() {
    for code in heldKeys { try? post(CGEvent(keyboardEventSource: source, virtualKey: code, keyDown: false)) }
    for button in heldButtons {
        let types: [CGEventType] = [.leftMouseUp,.rightMouseUp,.otherMouseUp]
        try? post(CGEvent(mouseEventSource: source, mouseType: types[Int(button)], mouseCursorPosition: sequencePoint ?? CGEvent(source:nil)?.location ?? .zero, mouseButton: CGMouseButton(rawValue: button)!))
    }
    heldKeys.removeAll(); heldButtons.removeAll(); sequencePoint = nil
}
@MainActor
func pacedInput<T>(_ items: [T], checkpoint: () throws -> Void, send: (T) throws -> Void) async throws {
    for item in items {
        // Bound the event backlog and let the physical-input monitor run.
        try await Task.sleep(nanoseconds: 5_000_000)
        try checkpoint()
        try send(item)
    }
}

@MainActor
func handle(_ request: [String: Any], checkpoint: () throws -> Void = {}) async throws -> [String: Any] {
    switch request["action"] as? String {
    case "info":
        let bounds = try desktopLayout().bounds
        let point = CGEvent(source:nil)?.location ?? .zero
        var result: [String: Any] = ["width":Int(bounds.width),"height":Int(bounds.height),
            "x":Int(point.x-bounds.minX),"y":Int(point.y-bounds.minY),
            "screen_recording":CGPreflightScreenCaptureAccess(),"accessibility":CGPreflightPostEventAccess(),"input_monitoring":CGPreflightListenEventAccess()]
        if let app = NSWorkspace.shared.frontmostApplication, !app.isTerminated {
            result["pid"] = app.processIdentifier
            if let id = app.bundleIdentifier { result["app_id"] = id }
        }
        return result
    case "view":
        guard CGPreflightScreenCaptureAccess() else { throw NativeError.denied }
        let layout = try desktopLayout()
        let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: true)
        guard let canvas = CGContext(data: nil, width: Int(layout.bounds.width), height: Int(layout.bounds.height),
                                     bitsPerComponent: 8, bytesPerRow: 0, space: CGColorSpace(name: CGColorSpace.sRGB)!,
                                     bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { throw NativeError.unavailable }
        canvas.setFillColor(CGColor(gray: 0, alpha: 1)); canvas.fill(CGRect(origin: .zero, size: layout.bounds.size))
        for geometry in layout.displays {
            try checkpoint()
            guard let display = content.displays.first(where: {$0.displayID == geometry.id}) else { throw NativeError.unavailable }
            let configuration = SCStreamConfiguration()
            configuration.width = Int(geometry.bounds.width); configuration.height = Int(geometry.bounds.height)
            configuration.showsCursor = true
            let image = try await SCScreenshotManager.captureImage(contentFilter: SCContentFilter(display:display,excludingWindows:[]), configuration:configuration)
            try checkpoint()
            canvas.draw(image, in: layout.imageRect(geometry))
        }
        guard try desktopLayout() == layout, let image = canvas.makeImage(),
              let png = NSBitmapImageRep(cgImage:image).representation(using:.png,properties:[:]) else { throw NativeError.unavailable }
        return ["png":png.base64EncodedString()]
    case "pointer":
        guard CGPreflightPostEventAccess(), CGPreflightListenEventAccess(), let steps = request["steps"] as? [[String:Any]], steps.count <= 4096 else { throw NativeError.denied }
        // Keep one coordinate origin for the sequence. Checkpoints reject a changed layout;
        // never reinterpret an earlier frame's coordinates against a newly attached display.
        let layout = try desktopLayout()
        try await pacedInput(steps, checkpoint: checkpoint, send: { try pointer($0, layout: layout) })
        return [:]
    case "keyboard":
        guard CGPreflightPostEventAccess(), CGPreflightListenEventAccess(), let strokes = request["strokes"] as? [[String:Any]], strokes.count <= 4096 else { throw NativeError.denied }
        try await pacedInput(strokes, checkpoint: checkpoint, send: key)
        return [:]
    default: throw NativeError.invalid
    }
}
struct DesktopAccessControl: Equatable {
    let enabled: Bool
    let title: String
}

func desktopAccessControl(started: Bool, sessionOpen: Bool, allowed: Bool, paused: Bool, restartRequired: Bool) -> DesktopAccessControl {
    if restartRequired { return DesktopAccessControl(enabled: false, title: "Restart Visor to recover") }
    return DesktopAccessControl(enabled: started && sessionOpen && !allowed,
                                title: paused ? "Resume desktop access" : "Allow desktop access")
}

@MainActor final class SetupController: NSObject, NSWindowDelegate {
    var window: NSWindow!
    var status: NSTextField!
    func show() {
        window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 680, height: 470),
                          styleMask: [.titled, .closable, .miniaturizable], backing: .buffered, defer: false)
        window.title = "Hypermesh Visor Development"
        window.isReleasedWhenClosed = false
        window.delegate = self
        let content = window.contentView!
        let intro = NSTextField(wrappingLabelWithString: "Desktop access starts off. While allowed, local clients can capture and control this desktop. Your mouse or keyboard input pauses it.")
        intro.frame = NSRect(x: 24, y: 205, width: 552, height: 52)
        content.addSubview(intro)
        status = NSTextField(wrappingLabelWithString: "")
        status.frame = NSRect(x: 24, y: 133, width: 552, height: 60)
        content.addSubview(status)
        for (index, pair) in [("Screen Recording", #selector(screen)), ("Accessibility", #selector(accessibility)), ("Input Monitoring", #selector(input))].enumerated() {
            let button = NSButton(title: pair.0, target: self, action: pair.1)
            button.frame = NSRect(x: 24 + index * 185, y: 85, width: 175, height: 32)
            content.addSubview(button)
        }
        let refreshButton = NSButton(title: "Refresh Status", target: self, action: #selector(refresh))
        refreshButton.frame = NSRect(x: 24, y: 30, width: 150, height: 32)
        content.addSubview(refreshButton)
        let quitButton = NSButton(title: "Quit", target: NSApplication.shared, action: #selector(NSApplication.terminate(_:)))
        quitButton.frame = NSRect(x: 470, y: 30, width: 100, height: 32)
        content.addSubview(quitButton)
        #if EMBEDDED_ENGINE
        let native = NativeRuntime.shared
        NSApplication.shared.delegate = native
        let runtimeLabel = NSTextField(wrappingLabelWithString: "")
        runtimeLabel.frame = NSRect(x: 24, y: 290, width: 630, height: 100)
        content.addSubview(runtimeLabel); native.label = runtimeLabel
        for (index, pair) in [("Allow desktop access", #selector(NativeRuntime.allow)), ("Pause", #selector(NativeRuntime.pauseButton)), ("Stop", #selector(NativeRuntime.stopButton))].enumerated() {
            let button = NSButton(title: pair.0, target: native, action: pair.1)
            button.frame = NSRect(x: 24 + index * 210, y: 400, width: 200, height: 32)
            content.addSubview(button)
            if index == 0 { native.allowButton = button }
        }
        native.start()
        #endif
        refresh()
        Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] _ in
            Task { @MainActor in self?.refresh() }
        }
        window.center(); window.makeKeyAndOrderFront(nil)
        NSApplication.shared.activate(ignoringOtherApps: true)
    }
    func windowShouldClose(_ sender: NSWindow) -> Bool {
        #if EMBEDDED_ENGINE
        NativeRuntime.shared.pause("Control window closed")
        #endif
        NSApplication.shared.terminate(nil)
        return false
    }
    @objc func refresh() {
        status.stringValue = "Screen Recording: \(CGPreflightScreenCaptureAccess() ? "allowed" : "not allowed")\nAccessibility: \(CGPreflightPostEventAccess() ? "allowed" : "not allowed")\nInput Monitoring: \(CGPreflightListenEventAccess() ? "allowed" : "not allowed")"
    }
    func settings(_ pane: String) {
        if let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?" + pane) {
            NSWorkspace.shared.open(url)
        }
        refresh()
    }
    @objc func screen() { _ = CGRequestScreenCaptureAccess(); settings("Privacy_ScreenCapture") }
    @objc func accessibility() { _ = CGRequestPostEventAccess(); settings("Privacy_Accessibility") }
    @objc func input() { _ = CGRequestListenEventAccess(); settings("Privacy_ListenEvent") }
}
@MainActor func runSetup() {
    let app = NSApplication.shared
    app.setActivationPolicy(.regular)
    let controller = SetupController()
    controller.show()
    withExtendedLifetime(controller) { app.run() }
}
func verifyNativeUnit(_ condition: @autoclosure () -> Bool, line: UInt = #line) throws {
    if !condition() {
        FileHandle.standardError.write(Data("FAIL native unit at Desktop.swift:\(line)\n".utf8))
        throw NativeError.invalid
    }
}
func nativeMascotState(connected: Bool, allowed: Bool, phase: String) -> String {
    if !connected { return "disconnected" }
    return allowed && phase == "running" ? "automation" : "idle"
}

func nativeUsageSummary(_ session: [String:Any]?) -> String? {
    guard let usage = session?["last_response_usage"] as? [String:Any],
          let input = usage["input_tokens"] as? NSNumber,
          let output = usage["output_tokens"] as? NSNumber,
          CFGetTypeID(input) != CFBooleanGetTypeID(), CFGetTypeID(output) != CFBooleanGetTypeID(),
          input.doubleValue >= 0, output.doubleValue >= 0 else { return nil }
    return "Latest response · Input \(input.uint64Value) · Output \(output.uint64Value) tokens"
}

func selfTest() throws {
    try verifyNativeUnit(nativeMascotState(connected: false, allowed: true, phase: "running") == "disconnected")
    try verifyNativeUnit(nativeMascotState(connected: true, allowed: true, phase: "running") == "automation")
    try verifyNativeUnit(nativeMascotState(connected: true, allowed: true, phase: "paused") == "idle")
    try verifyNativeUnit(nativeMascotState(connected: true, allowed: false, phase: "running") == "idle")
    try verifyNativeUnit(nativeUsageSummary(nil) == nil)
    try verifyNativeUnit(nativeUsageSummary(["last_response_usage": ["input_tokens": true, "output_tokens": 1]]) == nil)
    try verifyNativeUnit(nativeUsageSummary(["last_response_usage": ["input_tokens": 0, "output_tokens": 0]]) == "Latest response · Input 0 · Output 0 tokens")
    print("PASS native mascot state and reported usage contracts")
    for started in [false, true] {
        for sessionOpen in [false, true] {
            for allowed in [false, true] {
                for paused in [false, true] {
                    let lost = desktopAccessControl(started: started, sessionOpen: sessionOpen, allowed: allowed, paused: paused, restartRequired: true)
                    try verifyNativeUnit(!lost.enabled && lost.title == "Restart Visor to recover")
                    let healthy = desktopAccessControl(started: started, sessionOpen: sessionOpen, allowed: allowed, paused: paused, restartRequired: false)
                    try verifyNativeUnit(healthy.enabled == (started && sessionOpen && !allowed))
                    try verifyNativeUnit(healthy.title == (paused ? "Resume desktop access" : "Allow desktop access"))
                }
            }
        }
    }
    print("PASS permanent monitor loss requires restart; ordinary pauses remain resumable")
    try verifyNativeUnit(desktopInterruption(permissionsAndMonitor: true, layoutMatches: true) == nil)
    try verifyNativeUnit(desktopInterruption(permissionsAndMonitor: true, layoutMatches: false)?.permanent == false)
    for layoutMatches in [true, false] {
        try verifyNativeUnit(desktopInterruption(permissionsAndMonitor: false, layoutMatches: layoutMatches)?.permanent == true)
    }
    print("PASS layout-only interruption is recoverable; monitor loss remains permanent")
    let primary = DisplayGeometry(id: 1, bounds: CGRect(x: 0, y: 0, width: 1920, height: 1080))
    let left = DisplayGeometry(id: 2, bounds: CGRect(x: -1280, y: -200, width: 1280, height: 1024))
    let layout = try DesktopLayout([primary, left])
    try verifyNativeUnit(layout.bounds == CGRect(x: -1280, y: -200, width: 3200, height: 1280))
    let topLeft = try layout.point(x: 0, y: 0)
    try verifyNativeUnit(topLeft == CGPoint(x: -1280, y: -200))
    let primaryOrigin = try layout.point(x: 1280, y: 200)
    try verifyNativeUnit(primaryOrigin == .zero)
    try verifyNativeUnit(layout.imageRect(left) == CGRect(x: 0, y: 256, width: 1280, height: 1024))
    try verifyNativeUnit(layout.imageRect(primary) == CGRect(x: 1280, y: 0, width: 1920, height: 1080))
    for (x, y) in [(0, 1279), (1280, 0), (-1, 0), (3200, 200)] {
        do { _ = try layout.point(x: x, y: y); preconditionFailure("gap or outside point accepted") } catch NativeError.invalid { }
    }
    do { _ = try DesktopLayout([DisplayGeometry(id: 1, bounds: CGRect(x: 0, y: 0, width: 10000, height: 10000))]); preconditionFailure("oversized layout accepted") } catch NativeError.unavailable { }
    let reordered = try DesktopLayout([left, primary])
    try verifyNativeUnit(reordered == layout)
    let below = DisplayGeometry(id: 1, bounds: CGRect(x: 0, y: 0, width: 2, height: 2))
    let above = DisplayGeometry(id: 2, bounds: CGRect(x: 0, y: -2, width: 2, height: 2))
    let vertical = try DesktopLayout([below, above])
    let canvas = CGContext(data: nil, width: 2, height: 4, bitsPerComponent: 8, bytesPerRow: 0,
                          space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    canvas.setFillColor(CGColor(colorSpace: CGColorSpace(name: CGColorSpace.sRGB)!, components: [1, 0, 0, 1])!); canvas.fill(vertical.imageRect(below))
    let rows = Data([0,255,0,255, 0,255,0,255, 0,0,255,255, 0,0,255,255])
    let fixture = CGImage(width: 2, height: 2, bitsPerComponent: 8, bitsPerPixel: 32, bytesPerRow: 8,
                          space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.premultipliedLast.rawValue),
                          provider: CGDataProvider(data: rows as CFData)!, decode: nil, shouldInterpolate: false, intent: .defaultIntent)!
    canvas.draw(fixture, in: vertical.imageRect(above))
    let encoded = NSBitmapImageRep(cgImage: canvas.makeImage()!).representation(using: .png, properties: [:])!
    let bitmap = NSBitmapImageRep(data: encoded)!
    let reference = NSBitmapImageRep(data: NSBitmapImageRep(cgImage: fixture).representation(using: .png, properties: [:])!)!
    func pixel(_ rep: NSBitmapImageRep, _ y: Int) -> [Int] {
        var samples = [Int](repeating: 0, count: rep.samplesPerPixel)
        rep.getPixel(&samples, atX: 0, y: y)
        return samples
    }
    // Compare encoded pixels, not NSColor's display-dependent floating-point conversion.
    let top = pixel(bitmap, 0), second = pixel(bitmap, 1), bottom = pixel(bitmap, 3)
    FileHandle.standardError.write(Data("Synthetic PNG rows: \(top), \(second), \(bottom); source: \(pixel(reference, 0)), \(pixel(reference, 1))\n".utf8))
    try verifyNativeUnit(top == pixel(reference, 0))
    try verifyNativeUnit(second == pixel(reference, 1))
    try verifyNativeUnit(top != second && bottom != top && bottom != second)
    print("PASS multi-display logical coordinates, gaps, composition placement and bounds")
    for (button, expected) in [(UInt32(0), CGEventType.leftMouseDragged), (1, .rightMouseDragged), (2, .otherMouseDragged)] {
        let event = moveEvent(point: .zero, buttons: [button])!
        try verifyNativeUnit(event.type == expected)
        try verifyNativeUnit(event.getIntegerValueField(.mouseEventButtonNumber) == Int64(button))
    }
    try verifyNativeUnit(moveEvent(point: .zero, buttons: [])!.type == .mouseMoved)
    for (character, expected) in [("1",18),(" ",49),("-",27),("/",44),(";",41),("a",0)] {
        let (event, _) = try keyEvent(["keysym":Int(character.unicodeScalars.first!.value),"down":true], held:[55])
        try verifyNativeUnit(event.getIntegerValueField(.keyboardEventKeycode) == expected)
        try verifyNativeUnit(event.flags.contains(.maskCommand))
    }
    let (shifted, _) = try keyEvent(["keysym":Int(Character("!").asciiValue!),"down":true], held:[55])
    try verifyNativeUnit(shifted.getIntegerValueField(.keyboardEventKeycode) == 18 && shifted.flags.contains(.maskShift))
    let emoji = 0x01000000 + 0x1f642
    let (text, _) = try keyEvent(["keysym":emoji,"down":true], held:[])
    var count = 0
    var chars = [UniChar](repeating:0,count:8)
    text.keyboardGetUnicodeString(maxStringLength:8,actualStringLength:&count,unicodeString:&chars)
    try verifyNativeUnit(String(utf16CodeUnits:chars,count:count) == "🙂")
    do {
        _ = try keyEvent(["keysym":emoji,"down":true], held:[55])
        preconditionFailure("unsupported physical shortcut accepted")
    } catch NativeError.invalid { }
    print("PASS native drag button identity, physical shortcuts, Unicode text, unsupported shortcut refusal")
}
@main struct NativeDesktop {
    @MainActor static func main() async {
        let arguments = Array(CommandLine.arguments.dropFirst())
        if arguments == ["--self-test"] {
            do {
                try selfTest()
                var delivered = [Int]()
                do {
                    try await pacedInput([1, 2, 3], checkpoint: {
                        if !delivered.isEmpty { throw NativeError.denied }
                    }, send: { delivered.append($0) })
                    preconditionFailure("cancelled sequence completed")
                } catch NativeError.denied { }
                precondition(delivered == [1])
                print("PASS cancellation checkpoint prevents later events without posting input")
            } catch { exit(1) }
            return
        }
        if arguments == ["--setup"] || (arguments.isEmpty && Bundle.main.bundleIdentifier == "sh.hyperme.visor.development") {
            runSetup()
            return
        }
        if arguments == ["--permissions"] {
            _ = CGRequestScreenCaptureAccess(); _ = CGRequestPostEventAccess(); _ = CGRequestListenEventAccess()
            return
        }
        // Never expose this TCC-authorized app as a generic stdin/stdout deputy.
        // Restore automation only after an authenticated daemon-owned transport exists.
        FileHandle.standardOutput.write(Data("{\"error\":\"Native automation disabled pending authenticated daemon integration.\"}\n".utf8))
        exit(1)
    }
}
