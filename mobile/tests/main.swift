import Foundation

final class FixtureStore: NativeSessionStore, @unchecked Sendable {
    private let lock = NSLock()
    private var token: String?
    let unavailable: Bool
    let unexpected: Bool

    init(unavailable: Bool = false, unexpected: Bool = false) {
        self.unavailable = unavailable
        self.unexpected = unexpected
    }
    func readRefreshToken() throws -> String? {
        lock.lock(); defer { lock.unlock() }
        if unexpected { throw NSError(domain: "fixture-sensitive-detail", code: 1) }
        if unavailable { throw StoreError.Unavailable }
        return token
    }
    func writeRefreshToken(token: String) throws {
        lock.lock(); defer { lock.unlock() }
        if unexpected { throw NSError(domain: "fixture-sensitive-detail", code: 1) }
        if unavailable { throw StoreError.Unavailable }
        self.token = token
    }
    func deleteRefreshToken() throws {
        lock.lock(); defer { lock.unlock() }
        if unexpected { throw NSError(domain: "fixture-sensitive-detail", code: 1) }
        if unavailable { throw StoreError.Unavailable }
        token = nil
    }
}

func require(_ condition: Bool) {
    precondition(condition, "mobile callback contract failed")
}

let store = FixtureStore()
let session = MobileSession(store: store)
require(try !session.hasStoredSession())
try session.storeRefreshToken(token: "fixture-one")
require(try session.hasStoredSession())
require(try store.readRefreshToken() == "fixture-one")
try session.storeRefreshToken(token: "fixture-two")
require(try store.readRefreshToken() == "fixture-two")
do {
    try session.storeRefreshToken(token: "")
    fatalError("empty credential accepted")
} catch StoreError.InvalidCredential {}
require(try store.readRefreshToken() == "fixture-two")
try store.deleteRefreshToken()
require(try !session.hasStoredSession())
try session.storeRefreshToken(token: "fixture-three")
try session.clearLocalSession()
require(try store.readRefreshToken() == nil)

for fixture in [FixtureStore(unavailable: true), FixtureStore(unexpected: true)] {
    let unavailable = MobileSession(store: fixture)
    do {
        _ = try unavailable.hasStoredSession()
        fatalError("unavailable store accepted")
    } catch StoreError.Unavailable {}
    do {
        try unavailable.storeRefreshToken(token: "fixture")
        fatalError("unavailable write accepted")
    } catch StoreError.Unavailable {}
    do {
        try unavailable.clearLocalSession()
        fatalError("unavailable delete accepted")
    } catch StoreError.Unavailable {}
}
print("PASS mobile Swift callback contract")

try keychainContract()
