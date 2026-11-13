import Foundation
import LocalAuthentication
import Security

// The app owns this adapter. Generated bindings and this file share a module.
// No access group, iCloud synchronization, filesystem fallback or token cache.
protocol SessionKeychain {
    func copy(_ query: [String: Any]) -> (OSStatus, Any?)
    func add(_ attributes: [String: Any]) -> OSStatus
    func update(_ query: [String: Any], _ attributes: [String: Any]) -> OSStatus
    func delete(_ query: [String: Any]) -> OSStatus
}

private struct SystemSessionKeychain: SessionKeychain {
    func copy(_ query: [String: Any]) -> (OSStatus, Any?) {
        var value: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &value)
        return (status, value)
    }
    func add(_ attributes: [String: Any]) -> OSStatus {
        SecItemAdd(attributes as CFDictionary, nil)
    }
    func update(_ query: [String: Any], _ attributes: [String: Any]) -> OSStatus {
        SecItemUpdate(query as CFDictionary, attributes as CFDictionary)
    }
    func delete(_ query: [String: Any]) -> OSStatus {
        SecItemDelete(query as CFDictionary)
    }
}

final class KeychainSessionStore: NativeSessionStore, @unchecked Sendable {
    private let lock = NSLock()
    private let service: String
    private let account: String
    private let keychain: SessionKeychain

    convenience init(service: String, account: String) throws {
        try self.init(service: service, account: account, keychain: SystemSessionKeychain())
    }

    // Internal injection is only for native contract tests, not runtime settings.
    init(service: String, account: String, keychain: SessionKeychain) throws {
        guard Self.valid(service, limit: 128), Self.valid(account, limit: 256) else {
            throw StoreError.Unavailable
        }
        self.service = service
        self.account = account
        self.keychain = keychain
    }

    private static func valid(_ value: String, limit: Int = 65536) -> Bool {
        !value.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty &&
            value.utf8.count <= limit && !value.utf8.contains { $0 < 32 || $0 == 127 }
    }

    private func query() -> [String: Any] {
        let authentication = LAContext()
        authentication.interactionNotAllowed = true
        return [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecAttrSynchronizable as String: false,
            kSecUseDataProtectionKeychain as String: true,
            kSecUseAuthenticationContext as String: authentication,
        ]
    }

    func readRefreshToken() throws -> String? {
        lock.lock(); defer { lock.unlock() }
        var request = query()
        request[kSecMatchLimit as String] = kSecMatchLimitOne
        request[kSecReturnData as String] = true
        let (status, value) = keychain.copy(request)
        if status == errSecItemNotFound { return nil }
        guard status == errSecSuccess, let data = value as? Data,
              data.count <= 65536, let token = String(data: data, encoding: .utf8),
              Self.valid(token) else { throw StoreError.Unavailable }
        return token
    }

    func writeRefreshToken(token: String) throws {
        guard Self.valid(token) else { throw StoreError.InvalidCredential }
        lock.lock(); defer { lock.unlock() }
        let request = query()
        let attributes: [String: Any] = [
            kSecValueData as String: Data(token.utf8),
            kSecAttrAccessible as String: kSecAttrAccessibleWhenUnlockedThisDeviceOnly,
        ]
        var status = keychain.update(request, attributes)
        if status == errSecItemNotFound {
            status = keychain.add(request.merging(attributes) { _, new in new })
            if status == errSecDuplicateItem {
                status = keychain.update(request, attributes)
            }
        }
        guard status == errSecSuccess else { throw StoreError.Unavailable }
    }

    func deleteRefreshToken() throws {
        lock.lock(); defer { lock.unlock() }
        let status = keychain.delete(query())
        guard status == errSecSuccess || status == errSecItemNotFound else {
            throw StoreError.Unavailable
        }
    }
}
