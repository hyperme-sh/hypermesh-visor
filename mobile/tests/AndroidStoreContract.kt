package sh.hyperme.visor.mobile

import java.nio.channels.FileChannel
import java.nio.file.Files
import java.nio.file.StandardOpenOption.WRITE
import java.nio.file.attribute.PosixFilePermissions
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import uniffi.hypermesh_visor_mobile.MobileSession
import uniffi.hypermesh_visor_mobile.StoreException

private class FixtureKeys : SessionKeys {
    var key: SecretKey? = null
    var unavailable = false
    var creates = 0
    override fun requireAvailable() { if (unavailable) error("fixture-sensitive-provider-detail") }
    override fun existing(): SecretKey? { requireAvailable(); return key }
    override fun create(): SecretKey { requireAvailable(); creates++; return KeyGenerator.getInstance("AES").apply { init(256) }.generateKey().also { key = it } }
    override fun delete() { requireAvailable(); key = null }
}

private fun refused(action: () -> Unit) {
    try { action(); error("unavailable storage accepted") } catch (error: StoreException.Unavailable) {
        check(!error.toString().contains("sensitive-provider"))
    }
}

fun androidStoreContract() {
    val root = Files.createTempDirectory("visor-mobile-contract-")
    val directory = root.resolve("private")
    val identity = "a".repeat(64)
    val keys = FixtureKeys()
    val store = EncryptedSessionStore(directory, identity, keys)
    val file = directory.resolve("$identity.bin")
    try {
        MobileSession(store).use { session ->
            check(!session.hasStoredSession())
            session.storeRefreshToken("fixture-secret-token")
            check(session.hasStoredSession())
            check(store.readRefreshToken() == "fixture-secret-token")
            check(keys.creates == 1)
            val first = Files.readAllBytes(file)
            check(!String(first, Charsets.ISO_8859_1).contains("fixture-secret-token"))
            check(Files.getPosixFilePermissions(file) == PosixFilePermissions.fromString("rw-------"))
            session.storeRefreshToken("fixture-secret-token")
            check(!first.contentEquals(Files.readAllBytes(file))) // Fresh provider-generated IV.
            val original = Files.readAllBytes(file)
            keys.unavailable = true
            refused { session.hasStoredSession() }
            refused { session.storeRefreshToken("replacement") }
            refused { session.clearLocalSession() }
            check(original.contentEquals(Files.readAllBytes(file)))
            keys.unavailable = false
            check(store.readRefreshToken() == "fixture-secret-token")

            val oldKey = keys.key
            keys.key = null
            refused { session.hasStoredSession() }
            refused { session.storeRefreshToken("must-not-reset") }
            check(keys.creates == 1 && original.contentEquals(Files.readAllBytes(file)))
            keys.key = oldKey
            for (blob in listOf(ByteArray(0), original.copyOf(10), original + ByteArray(70000), original.clone().apply { this[lastIndex] = (this[lastIndex].toInt() xor 1).toByte() })) {
                Files.write(file, blob)
                refused { session.hasStoredSession() }
            }
            Files.write(file, original)
            val other = "b".repeat(64)
            Files.write(directory.resolve("$other.bin"), original)
            refused { EncryptedSessionStore(directory, other, keys).readRefreshToken() }
            Files.delete(directory.resolve("$other.bin"))
            Files.deleteIfExists(directory.resolve("$other.lock"))

            FileChannel.open(directory.resolve("$identity.lock"), WRITE).use { channel ->
                val lock = channel.lock()
                try { refused { session.storeRefreshToken("busy") } } finally { lock.release() }
            }
            check(original.contentEquals(Files.readAllBytes(file)))
            for (token in listOf("", " ", "\u0000bad", "a".repeat(65537), "\uD800")) {
                try { store.writeRefreshToken(token); error("invalid token accepted") } catch (_: StoreException.InvalidCredential) {}
            }
            check(original.contentEquals(Files.readAllBytes(file)))
            val symlinkTarget = root.resolve("unrelated")
            Files.write(symlinkTarget, "keep".toByteArray())
            Files.delete(file)
            Files.createSymbolicLink(file, symlinkTarget)
            refused { session.hasStoredSession() }
            refused { session.storeRefreshToken("unsafe") }
            refused { session.clearLocalSession() }
            check(String(Files.readAllBytes(symlinkTarget)) == "keep")
            Files.delete(file)
            Files.write(file, original)
            session.clearLocalSession()
            check(!Files.exists(file) && keys.key == null)
            session.clearLocalSession()
            check(!session.hasStoredSession())
            session.storeRefreshToken("after-clear")
            check(keys.creates == 2 && store.readRefreshToken() == "after-clear")
            Files.setPosixFilePermissions(directory, PosixFilePermissions.fromString("rwxr-xr-x"))
            refused { session.hasStoredSession() }
            Files.setPosixFilePermissions(directory, PosixFilePermissions.fromString("rwx------"))
        }
    } finally {
        Files.walk(root).use { paths -> paths.sorted(Comparator.reverseOrder()).forEach { Files.deleteIfExists(it) } }
    }
    println("PASS Android encrypted-store contract (fixture keys, JVM)")
}
