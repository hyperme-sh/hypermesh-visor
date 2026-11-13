import uniffi.hypermesh_visor_mobile.MobileSession
import uniffi.hypermesh_visor_mobile.NativeSessionStore
import uniffi.hypermesh_visor_mobile.StoreException

private class FixtureStore(private val unavailable: Boolean = false) : NativeSessionStore {
    private var token: String? = null
    @Synchronized override fun readRefreshToken(): String? {
        if (unavailable) throw StoreException.Unavailable()
        return token
    }
    @Synchronized override fun writeRefreshToken(token: String) {
        if (unavailable) throw StoreException.Unavailable()
        this.token = token
    }
    @Synchronized override fun deleteRefreshToken() {
        if (unavailable) throw StoreException.Unavailable()
        token = null
    }
}

fun main() {
    val store = FixtureStore()
    MobileSession(store).use { session ->
        check(!session.hasStoredSession())
        session.storeRefreshToken("fixture-one")
        check(session.hasStoredSession())
        check(store.readRefreshToken() == "fixture-one")
        session.storeRefreshToken("fixture-two")
        try {
            session.storeRefreshToken("")
            error("empty credential accepted")
        } catch (_: StoreException.InvalidCredential) {}
        check(store.readRefreshToken() == "fixture-two")
        store.deleteRefreshToken()
        check(!session.hasStoredSession())
        session.storeRefreshToken("fixture-three")
        session.clearLocalSession()
        check(store.readRefreshToken() == null)
    }
    MobileSession(FixtureStore(unavailable = true)).use { session ->
        try {
            session.hasStoredSession()
            error("unavailable store accepted")
        } catch (_: StoreException.Unavailable) {}
        try {
            session.storeRefreshToken("fixture")
            error("unavailable write accepted")
        } catch (_: StoreException.Unavailable) {}
        try {
            session.clearLocalSession()
            error("unavailable delete accepted")
        } catch (_: StoreException.Unavailable) {}
    }
    println("PASS mobile Kotlin callback contract")
}
