import Foundation
import LocalAuthentication
import Security

private final class RecordingKeychain: SessionKeychain {
    var value: Any?
    var failure: OSStatus?
    var raceOnAdd = false
    var calls = 0
    var deletes = 0
    var adds = 0
    var updates = 0

    func check(_ query: [String: Any]) {
        calls += 1
        precondition(query[kSecClass as String] as? String == kSecClassGenericPassword as String)
        precondition(query[kSecAttrService as String] as? String == "fixture-service")
        precondition(query[kSecAttrAccount as String] as? String == "fixture-account")
        precondition(query[kSecAttrSynchronizable as String] as? Bool == false)
        precondition(query[kSecUseDataProtectionKeychain as String] as? Bool == true)
        precondition((query[kSecUseAuthenticationContext as String] as? LAContext)?.interactionNotAllowed == true)
        precondition(query[kSecAttrAccessGroup as String] == nil)
    }
    func checkAttributes(_ attributes: [String: Any]) {
        precondition(attributes[kSecAttrAccessible as String] as? String == kSecAttrAccessibleWhenUnlockedThisDeviceOnly as String)
        precondition(attributes[kSecValueData as String] is Data)
    }
    func copy(_ query: [String: Any]) -> (OSStatus, Any?) {
        check(query)
        precondition(query[kSecMatchLimit as String] as? String == kSecMatchLimitOne as String)
        precondition(query[kSecReturnData as String] as? Bool == true)
        if let failure { return (failure, nil) }
        return (value == nil ? errSecItemNotFound : errSecSuccess, value)
    }
    func update(_ query: [String: Any], _ attributes: [String: Any]) -> OSStatus {
        check(query); checkAttributes(attributes); updates += 1
        if let failure { return failure }
        guard value != nil else { return errSecItemNotFound }
        value = attributes[kSecValueData as String]
        return errSecSuccess
    }
    func add(_ attributes: [String: Any]) -> OSStatus {
        check(attributes); checkAttributes(attributes); adds += 1
        if let failure { return failure }
        if raceOnAdd {
            value = Data("race-fixture".utf8)
            return errSecDuplicateItem
        }
        value = attributes[kSecValueData as String]
        return errSecSuccess
    }
    func delete(_ query: [String: Any]) -> OSStatus {
        check(query); deletes += 1
        if let failure { return failure }
        let status = value == nil ? errSecItemNotFound : errSecSuccess
        value = nil
        return status
    }
}

func keychainContract() throws {
    let backend = RecordingKeychain()
    let store = try KeychainSessionStore(service: "fixture-service", account: "fixture-account", keychain: backend)
    let session = MobileSession(store: store)
    require(try !session.hasStoredSession())
    try session.storeRefreshToken(token: "fixture-one")
    precondition(backend.adds == 1)
    try session.storeRefreshToken(token: "fixture-two")
    precondition(backend.adds == 1 && backend.deletes == 0)
    require(try store.readRefreshToken() == "fixture-two")
    backend.value = Data("external-change".utf8)
    require(try store.readRefreshToken() == "external-change")
    try session.clearLocalSession()
    try session.clearLocalSession()
    require(try !session.hasStoredSession())
    backend.raceOnAdd = true
    try session.storeRefreshToken(token: "after-race")
    require(try store.readRefreshToken() == "after-race")

    for status in [errSecInteractionNotAllowed, errSecAuthFailed, errSecNotAvailable, errSecMissingEntitlement] {
        backend.failure = status
        for operation in [{ _ = try session.hasStoredSession() }, { try session.storeRefreshToken(token: "fixture") }, { try session.clearLocalSession() }] {
            do { try operation(); fatalError("Keychain failure accepted") } catch StoreError.Unavailable {}
        }
    }
    backend.failure = nil
    for value in [Data([0xff]), Data(), Data("\u{0}bad".utf8), Data(repeating: 65, count: 65537), "not-data"] as [Any] {
        backend.value = value
        do { _ = try session.hasStoredSession(); fatalError("malformed Keychain value accepted") } catch StoreError.Unavailable {}
    }
    let before = backend.calls
    for token in ["", " ", "\u{0}bad", String(repeating: "a", count: 65537)] {
        do { try store.writeRefreshToken(token: token); fatalError("invalid token accepted") } catch StoreError.InvalidCredential {}
    }
    precondition(backend.calls == before)
    do { _ = try KeychainSessionStore(service: "", account: "fixture-account", keychain: backend); fatalError("empty namespace accepted") } catch StoreError.Unavailable {}
    print("PASS mobile Keychain adapter contract (recording backend)")
}
