import UIKit
import Capacitor

@UIApplicationMain
class AppDelegate: UIResponder, UIApplicationDelegate {

    var window: UIWindow?

    func application(_ application: UIApplication, didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]?) -> Bool {
        // Override point for customization after application launch.
        return true
    }

    func applicationWillResignActive(_ application: UIApplication) {
        // Sent when the application is about to move from active to inactive state. This can occur for certain types of temporary interruptions (such as an incoming phone call or SMS message) or when the user quits the application and it begins the transition to the background state.
        // Use this method to pause ongoing tasks, disable timers, and invalidate graphics rendering callbacks. Games should use this method to pause the game.
    }

    func applicationDidEnterBackground(_ application: UIApplication) {
        // Use this method to release shared resources, save user data, invalidate timers, and store enough application state information to restore your application to its current state in case it is terminated later.
        // If your application supports background execution, this method is called instead of applicationWillTerminate: when the user quits.
    }

    func applicationWillEnterForeground(_ application: UIApplication) {
        // Called as part of the transition from the background to the active state; here you can undo many of the changes made on entering the background.
    }

    func applicationDidBecomeActive(_ application: UIApplication) {
        // Restart any tasks that were paused (or not yet started) while the application was inactive. If the application was previously in the background, optionally refresh the user interface.
    }

    func applicationWillTerminate(_ application: UIApplication) {
        // Called when the application is about to terminate. Save data if appropriate. See also applicationDidEnterBackground:.
    }

    func application(_ application: UIApplication,
                     configurationForConnecting connectingSceneSession: UISceneSession,
                     options: UIScene.ConnectionOptions) -> UISceneConfiguration {
        let config = UISceneConfiguration(name: "Default Configuration",
                                          sessionRole: connectingSceneSession.role)
        config.delegateClass = SceneDelegate.self
        return config
    }
}

final class VisorBridgeViewController: CAPBridgeViewController {
    override func capacitorDidLoad() { bridge?.registerPluginInstance(VisorPlugin()) }
}

private enum PhoneFailure: Error { case unavailable }
private final class PhoneNativeState {
    let binding: [String]
    let owner: MobileOwnerSignIn
    let tunnel: PhoneTunnel
    let machine: MobileMachine
    let workspaceId: String
    let workspaceName: String
    init(issuer: String, auth: String, plane: String, workspace: String, name: String, conductor: String?) throws {
        guard UUID(uuidString: workspace)?.uuidString.lowercased() == workspace,
              !name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty, name.count <= 128 else {
            throw PhoneFailure.unavailable
        }
        binding = [issuer, auth, plane, workspace, name, conductor ?? ""]
        workspaceId = workspace; workspaceName = name
        machine = try appleMachineIdentity(service: "sh.hyperme.visor.mobile", ownerAccount: "device-owner-v1", planeOrigin: plane)
        owner = try appleOwnerSignIn(service: "sh.hyperme.visor.mobile", ownerAccount: "device-owner-v1", issuer: issuer, authOrigin: auth)
        tunnel = PhoneTunnel(machine: machine, owner: owner)
    }
}

@objc(VisorPlugin)
final class VisorPlugin: CAPPlugin, CAPBridgedPlugin {
    let identifier = "VisorPlugin"
    let jsName = "Visor"
    let pluginMethods: [CAPPluginMethod] = ["cancelPhone", "ownedSessions", "sendPrompt", "preparePrompt", "submitPrompt", "discardPrompt", "status", "beginSignIn", "finishSignIn", "cancelSignIn", "enroll", "renew", "revoke", "connect", "disconnect"].map {
        CAPPluginMethod(name: $0, returnType: CAPPluginReturnPromise)!
    }
    private static var shared: PhoneNativeState?
    private static let stateLock = NSLock()
    private var native: PhoneNativeState?
    private let worker = DispatchQueue(label: "sh.hyperme.visor.enrollment")
    private let busyLock = NSLock()
    private var busy = false

    override func load() {
        let config = getConfig()
        guard let issuer = config.getString("issuer"), let auth = config.getString("authOrigin"),
              let plane = config.getString("planeOrigin"), let workspace = config.getString("workspaceId"),
              let name = config.getString("workspaceName") else { return }
        let conductor = config.getString("conductorOrigin")
        Self.stateLock.lock(); defer { Self.stateLock.unlock() }
        do {
            if Self.shared == nil { Self.shared = try PhoneNativeState(issuer: issuer, auth: auth, plane: plane, workspace: workspace, name: name, conductor: conductor) }
            guard Self.shared?.binding == [issuer, auth, plane, workspace, name, conductor ?? ""] else { return }
            native = Self.shared
        } catch { /* Missing native storage/configuration never falls back to JS storage. */ }
    }
    private func run(_ call: CAPPluginCall, _ action: @escaping (PhoneNativeState) throws -> JSObject) {
        guard let state = native else { call.reject("Phone configuration is unavailable."); return }
        busyLock.lock()
        if busy { busyLock.unlock(); call.reject("Another action is in progress."); return }
        busy = true; busyLock.unlock()
        worker.async {
            let result = Result { try action(state) }
            self.busyLock.lock(); self.busy = false; self.busyLock.unlock()
            switch result {
            case .success(let value): call.resolve(value)
            case .failure: call.reject("That action could not be completed.")
            }
        }
    }
    private func publicState(_ state: PhoneNativeState) throws -> JSObject {
        let current = try state.machine.status()
        return ["enrolled": current?.enrolled == true && current?.revoked != true,
                "revoked": current?.revoked == true,
                "connection": state.tunnel.status(), "canConnect": current?.enrolled == true && current?.revoked != true]
    }
    @objc func status(_ call: CAPPluginCall) { run(call, publicState) }
    @objc func beginSignIn(_ call: CAPPluginCall) {
        run(call) { state in
            state.tunnel.disconnect()
            let prompt = try state.owner.begin()
            guard let url = URL(string: prompt.verificationUrl) else { throw PhoneFailure.unavailable }
            let completed = DispatchSemaphore(value: 0)
            let resultLock = NSLock()
            var opened = false
            DispatchQueue.main.async {
                UIApplication.shared.open(url, options: [:]) { success in
                    resultLock.lock(); opened = success; resultLock.unlock()
                    completed.signal()
                }
            }
            let timely = completed.wait(timeout: .now() + 10) == .success
            resultLock.lock(); let success = opened; resultLock.unlock()
            if !timely || !success { try state.owner.cancel(); throw PhoneFailure.unavailable }
            return ["userCode": prompt.userCode]
        }
    }
    @objc func finishSignIn(_ call: CAPPluginCall) {
        run(call) { state in
            try state.owner.finish()
            return ["workspaces": [["id": state.workspaceId, "name": state.workspaceName]]]
        }
    }
    @objc func cancelSignIn(_ call: CAPPluginCall) {
        // Cancellation must bypass the worker while it polls the device grant.
        do { native?.tunnel.disconnect(); try native?.owner.cancel(); call.resolve() }
        catch { call.reject("Sign-in could not be cancelled.") }
    }
    @objc func enroll(_ call: CAPPluginCall) {
        let selected = call.getString("workspaceId")
        run(call) { state in
            guard selected == state.workspaceId else { throw PhoneFailure.unavailable }
            if let prior = try state.machine.status() {
                if prior.revoked { try state.owner.revokeMachine(machine: state.machine, tenantId: prior.tenantId) }
                else if prior.enrolled { throw PhoneFailure.unavailable }
            }
            try state.owner.enrollMachine(machine: state.machine, tenantId: state.workspaceId, platform: "ios")
            return try self.publicState(state)
        }
    }
    @objc func renew(_ call: CAPPluginCall) {
        run(call) { state in try state.machine.renew(); return try self.publicState(state) }
    }
    @objc func revoke(_ call: CAPPluginCall) {
        native?.tunnel.disconnect()
        run(call) { state in
            guard let current = try state.machine.status() else { throw PhoneFailure.unavailable }
            try state.owner.revokeMachine(machine: state.machine, tenantId: current.tenantId)
            return try self.publicState(state)
        }
    }
    @objc func cancelPhone(_ call: CAPPluginCall) { native?.owner.cancelPhoneOperations(); call.resolve() }
    @objc func ownedSessions(_ call: CAPPluginCall) {
        run(call) { state in
            return ["sessions": try state.tunnel.mesh.ownedSessions(owner: state.owner).map { session in
                ["botId": session.botId, "assignmentId": session.assignmentId,
                 "sessionId": session.sessionId, "expiresAt": Int(session.expiresAt)] as JSObject
            }]
        }
    }
    @objc func sendPrompt(_ call: CAPPluginCall) {
        guard let bot = call.getString("botId"), let assignment = call.getString("assignmentId"), let session = call.getString("sessionId"),
              let taskClass = call.getString("taskClass"), let destination = call.getString("destination"), let prompt = call.getString("prompt"),
              let limit = call.getInt("maxOutputTokens"), let bounded = Int32(exactly: limit),
              let expires = call.getInt("expiresAt"), expires > 0 else { call.reject("Invalid prompt request."); return }
        run(call) { state in
            let phone = try state.tunnel.client()
            return ["text": try phone.sendOwned(owner: state.owner, prompt: PhonePrompt(botId: bot, assignmentId: assignment, sessionId: session,
                taskClass: taskClass, destination: destination, prompt: prompt, maxOutputTokens: bounded), expiresAt: Int64(expires))]
        }
    }
    @objc func preparePrompt(_ call: CAPPluginCall) {
        guard let bot = call.getString("botId"), let assignment = call.getString("assignmentId"), let session = call.getString("sessionId"),
              let taskClass = call.getString("taskClass"), let destination = call.getString("destination"), let prompt = call.getString("prompt"),
              let limit = call.getInt("maxOutputTokens"), let bounded = Int32(exactly: limit) else { call.reject("Invalid prompt request."); return }
        run(call) { state in
            let phone = try state.tunnel.client()
            let prepared = try phone.prepare(prompt: PhonePrompt(botId: bot, assignmentId: assignment, sessionId: session,
                taskClass: taskClass, destination: destination, prompt: prompt, maxOutputTokens: bounded))
            return ["operationId": prepared.operationId, "taskId": prepared.taskId, "requestSha256": prepared.requestSha256]
        }
    }
    @objc func submitPrompt(_ call: CAPPluginCall) {
        guard let id = call.getString("operationId") else { call.reject("Invalid prompt request."); return }
        run(call) { state in
            let phone = try state.tunnel.client()
            return ["text": try phone.submit(operationId: id)]
        }
    }
    @objc func discardPrompt(_ call: CAPPluginCall) {
        guard let id = call.getString("operationId") else { call.reject("Invalid prompt request."); return }
        run(call) { state in
            let phone = try state.tunnel.client()
            try phone.discard(operationId: id); return [:]
        }
    }
    @objc func connect(_ call: CAPPluginCall) { run(call) { state in try state.tunnel.connect(); return try self.publicState(state) } }
    @objc func disconnect(_ call: CAPPluginCall) { native?.tunnel.disconnect(); run(call, publicState) }
    deinit { native?.tunnel.disconnect(); try? native?.owner.cancel() }
}
