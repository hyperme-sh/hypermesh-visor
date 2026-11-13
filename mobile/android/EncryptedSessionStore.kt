package sh.hyperme.visor.mobile

import java.nio.ByteBuffer
import java.nio.charset.CodingErrorAction
import java.nio.channels.FileChannel
import java.nio.file.Files
import java.nio.file.LinkOption.NOFOLLOW_LINKS
import java.nio.file.Path
import java.nio.file.StandardCopyOption.ATOMIC_MOVE
import java.nio.file.StandardCopyOption.REPLACE_EXISTING
import java.nio.file.StandardOpenOption.CREATE
import java.nio.file.StandardOpenOption.READ
import java.nio.file.StandardOpenOption.WRITE
import java.nio.file.attribute.PosixFilePermissions
import javax.crypto.Cipher
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec
import uniffi.hypermesh_visor_mobile.NativeSessionStore
import uniffi.hypermesh_visor_mobile.StoreException

// Internal test seams are not application configuration or fallback providers.
internal interface SessionKeys {
    fun requireAvailable()
    fun existing(): SecretKey?
    fun create(): SecretKey
    fun delete()
}

internal class EncryptedSessionStore(
    private val directory: Path,
    private val identity: String,
    private val keys: SessionKeys,
) : NativeSessionStore {
    private val magic = byteArrayOf(72, 77, 86, 83, 1)
    private val maxToken = 65536
    private val maxBlob = maxToken + magic.size + 12 + 16
    private val file: Path
    private val lockFile: Path
    private val aad: ByteArray

    init {
        if (!identity.matches(Regex("[0-9a-f]{64}"))) throw StoreException.Unavailable()
        file = directory.resolve("$identity.bin")
        lockFile = directory.resolve("$identity.lock")
        aad = "hypermesh-visor-session-v1:$identity".toByteArray(Charsets.UTF_8)
    }

    private fun regularOrMissing(path: Path): Boolean {
        if (!Files.exists(path, NOFOLLOW_LINKS)) return false
        if (!Files.isRegularFile(path, NOFOLLOW_LINKS)) throw StoreException.Unavailable()
        return true
    }

    private fun <T> guarded(action: () -> T): T {
        try {
            keys.requireAvailable()
            if (!Files.exists(directory, NOFOLLOW_LINKS)) {
                try {
                    Files.createDirectory(directory, PosixFilePermissions.asFileAttribute(PosixFilePermissions.fromString("rwx------")))
                } catch (_: java.nio.file.FileAlreadyExistsException) { /* Validate the winner below. */ }
            }
            if (!Files.isDirectory(directory, NOFOLLOW_LINKS) ||
                Files.getPosixFilePermissions(directory, NOFOLLOW_LINKS) != PosixFilePermissions.fromString("rwx------")) {
                throw StoreException.Unavailable()
            }
            regularOrMissing(lockFile)
            FileChannel.open(lockFile, setOf(CREATE, WRITE, NOFOLLOW_LINKS),
                PosixFilePermissions.asFileAttribute(PosixFilePermissions.fromString("rw-------"))).use { channel ->
                val lock = channel.tryLock() ?: throw StoreException.Unavailable()
                try {
                    keys.requireAvailable()
                    return action()
                } finally { lock.release() }
            }
        } catch (_: Exception) {
            // Do not return paths, ciphertext, key metadata or provider diagnostics.
            throw StoreException.Unavailable()
        }
    }

    private fun boundedRead(): ByteArray {
        Files.newInputStream(file, READ, NOFOLLOW_LINKS).use { input ->
            val buffer = ByteArray(maxBlob + 1)
            var used = 0
            while (used < buffer.size) {
                val count = input.read(buffer, used, buffer.size - used)
                if (count < 0) break
                used += count
            }
            if (used > maxBlob) throw StoreException.Unavailable()
            return buffer.copyOf(used)
        }
    }

    override fun readRefreshToken(): String? = guarded {
        if (!regularOrMissing(file)) return@guarded null
        val key = keys.existing() ?: throw StoreException.Unavailable()
        val blob = boundedRead()
        if (blob.size <= magic.size + 12 + 16 || !blob.copyOfRange(0, magic.size).contentEquals(magic)) {
            throw StoreException.Unavailable()
        }
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.DECRYPT_MODE, key, GCMParameterSpec(128, blob.copyOfRange(magic.size, magic.size + 12)))
        cipher.updateAAD(aad)
        val plain = cipher.doFinal(blob, magic.size + 12, blob.size - magic.size - 12)
        try {
            val token = Charsets.UTF_8.newDecoder().onMalformedInput(CodingErrorAction.REPORT)
                .onUnmappableCharacter(CodingErrorAction.REPORT).decode(ByteBuffer.wrap(plain)).toString()
            if (!valid(token, plain)) throw StoreException.Unavailable()
            token
        } finally { plain.fill(0) }
    }

    private fun valid(token: String, bytes: ByteArray): Boolean =
        token.isNotBlank() && bytes.size <= maxToken && bytes.none { it.toInt() in 0..31 || it.toInt() == 127 }

    override fun writeRefreshToken(token: String) {
        if (token.length > maxToken) throw StoreException.InvalidCredential()
        val encoded = try {
            val encoded = Charsets.UTF_8.newEncoder().onMalformedInput(CodingErrorAction.REPORT)
                .onUnmappableCharacter(CodingErrorAction.REPORT).encode(java.nio.CharBuffer.wrap(token))
            ByteArray(encoded.remaining()).also { encoded.get(it) }
        } catch (_: Exception) { throw StoreException.InvalidCredential() }
        try {
            if (!valid(token, encoded)) throw StoreException.InvalidCredential()
            guarded {
                val present = regularOrMissing(file)
                val key = keys.existing() ?: if (present) throw StoreException.Unavailable() else keys.create()
                val cipher = Cipher.getInstance("AES/GCM/NoPadding")
                cipher.init(Cipher.ENCRYPT_MODE, key)
                if (cipher.iv.size != 12) throw StoreException.Unavailable()
                cipher.updateAAD(aad)
                val blob = magic + cipher.iv + cipher.doFinal(encoded)
                val temporary = Files.createTempFile(directory, ".session-", ".tmp",
                    PosixFilePermissions.asFileAttribute(PosixFilePermissions.fromString("rw-------")))
                try {
                    FileChannel.open(temporary, WRITE, NOFOLLOW_LINKS).use { channel ->
                        val bytes = ByteBuffer.wrap(blob)
                        while (bytes.hasRemaining()) channel.write(bytes)
                        channel.force(true)
                    }
                    keys.requireAvailable()
                    Files.move(temporary, file, ATOMIC_MOVE, REPLACE_EXISTING)
                } finally { Files.deleteIfExists(temporary) }
            }
        } finally { encoded.fill(0) }
    }

    override fun deleteRefreshToken() = guarded {
        regularOrMissing(file)
        Files.deleteIfExists(file)
        keys.delete()
    }
}
