package sh.hyperme.visor.mobile

import android.app.KeyguardManager
import android.content.Context
import android.os.Build
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import java.io.File
import java.security.KeyStore
import java.security.MessageDigest
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import uniffi.hypermesh_visor_mobile.NativeSessionStore
import uniffi.hypermesh_visor_mobile.StoreException

// The auth owner supplies a stable account namespace after account selection.
// This is storage only: it does not authenticate a token or enroll the device.
fun androidSessionStore(context: Context, account: String): NativeSessionStore {
    if (Build.VERSION.SDK_INT < 28 || context.isDeviceProtectedStorage || account.isBlank() ||
        account.length > 256 || !Charsets.UTF_8.newEncoder().canEncode(account) || account.any { it.code < 32 || it.code == 127 }) {
        throw StoreException.Unavailable()
    }
    val app = context.applicationContext
    val identity = MessageDigest.getInstance("SHA-256")
        .digest("${app.packageName}\u0000$account".toByteArray(Charsets.UTF_8))
        .joinToString("") { "%02x".format(it.toInt() and 255) }
    val manager = app.getSystemService(Context.KEYGUARD_SERVICE) as? KeyguardManager
        ?: throw StoreException.Unavailable()
    val directory = File(app.noBackupFilesDir, "visor-sessions").toPath()
    return EncryptedSessionStore(directory, identity, AndroidSessionKeys("hypermesh.visor.session.v1.$identity", manager))
}

private class AndroidSessionKeys(private val alias: String, private val manager: KeyguardManager) : SessionKeys {
    override fun requireAvailable() {
        if (manager.isDeviceLocked) throw StoreException.Unavailable()
    }
    private fun store(): KeyStore = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
    override fun existing(): SecretKey? {
        val keyStore = store()
        if (!keyStore.containsAlias(alias)) return null
        return keyStore.getKey(alias, null) as? SecretKey ?: throw StoreException.Unavailable()
    }
    override fun create(): SecretKey {
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
        generator.init(KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
            .setKeySize(256)
            .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
            .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
            .setRandomizedEncryptionRequired(true)
            .setUnlockedDeviceRequired(true)
            .build())
        return generator.generateKey()
    }
    override fun delete() { store().deleteEntry(alias) }
}

/** One app-owned instance per signed-in owner; separate from the owner's OAuth store. */
fun androidMachineIdentity(context: Context, ownerAccount: String, planeOrigin: String): uniffi.hypermesh_visor_mobile.MobileMachine {
    if (ownerAccount.isBlank() || ownerAccount.length > 256 || ownerAccount.any { it.code < 32 || it.code == 127 }) {
        throw StoreException.Unavailable()
    }
    val namespace = MessageDigest.getInstance("SHA-256")
        .digest((ownerAccount + "\u0000" + planeOrigin.trimEnd('/')).toByteArray(Charsets.UTF_8))
        .joinToString("") { "%02x".format(it.toInt() and 255) }
    return uniffi.hypermesh_visor_mobile.MobileMachine(
        androidSessionStore(context, "machine-v1:$namespace"), planeOrigin)
}

/** Browser authorization uses its own secure entry; credentials stay in the Rust core. */
fun androidOwnerSignIn(context: Context, ownerAccount: String, issuer: String, authOrigin: String): uniffi.hypermesh_visor_mobile.MobileOwnerSignIn {
    if (ownerAccount.isBlank() || ownerAccount.length > 256 || ownerAccount.any { it.code < 32 || it.code == 127 }) {
        throw StoreException.Unavailable()
    }
    val namespace = MessageDigest.getInstance("SHA-256")
        .digest((ownerAccount + "\u0000" + issuer + "\u0000" + authOrigin).toByteArray(Charsets.UTF_8))
        .joinToString("") { "%02x".format(it.toInt() and 255) }
    return uniffi.hypermesh_visor_mobile.MobileOwnerSignIn(
        androidSessionStore(context, "owner-v1:$namespace"), issuer, authOrigin)
}
