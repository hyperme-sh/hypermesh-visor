package sh.hyperme.visor.mobile.contract

import android.content.Context
import sh.hyperme.visor.mobile.androidSessionStore
import uniffi.hypermesh_visor_mobile.MobileSession

// Compiled against the packaged AAR, never against its source tree.
// This build contract is not installed or executed and writes no credential.
fun storedSession(context: Context, account: String): Boolean =
    MobileSession(androidSessionStore(context, account)).use { it.hasStoredSession() }
