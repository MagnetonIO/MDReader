package dev.mdreader.transfer

import android.app.Activity
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.system.Os
import android.system.OsConstants
import android.system.ErrnoException
import android.util.AtomicFile
import android.util.Base64
import app.tauri.annotation.InvokeArg
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import java.io.File
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

@InvokeArg class VaultRequest { lateinit var namespace: String; lateinit var reference: String; var bytes: String? = null }
/** Keystore AES-GCM wrapping key never leaves Android. Ciphertext is atomically
 * retained in noBackupFilesDir; loss/corruption never silently makes new keys. */
class SecureVault(private val activity: Activity) {
    private fun validate(args: VaultRequest) {
        require(args.namespace.matches(Regex("[a-zA-Z0-9._-]{1,200}")))
        require(args.reference.matches(Regex("native-sync-v1-[a-f0-9-]{36}")))
    }
    private fun file(args: VaultRequest): AtomicFile {
        val directory = File(activity.noBackupFilesDir, "mdreader-sync-vault-v1")
        require(directory.mkdir() || directory.isDirectory)
        require(directory.absoluteFile == directory.canonicalFile)
        Os.chmod(directory.path, 448) // 0700
        val root = Os.lstat(directory.path)
        require(OsConstants.S_ISDIR(root.st_mode) && root.st_uid == android.os.Process.myUid() && root.st_mode and 63 == 0)
        val path = File(directory, args.namespace + "." + args.reference)
        require(path.absoluteFile == path.canonicalFile)
        for (candidate in arrayOf(path, File(path.path + ".bak"), File(path.path + ".new"))) {
            try {
                val stat = Os.lstat(candidate.path)
                require(OsConstants.S_ISREG(stat.st_mode) && stat.st_nlink == 1L && stat.st_uid == android.os.Process.myUid())
                require(stat.st_mode and 63 == 0 && stat.st_size <= 32768)
            } catch (error: ErrnoException) { if (error.errno != OsConstants.ENOENT) throw error }
        }
        return AtomicFile(path)
    }
    private fun key(args: VaultRequest, create: Boolean): SecretKey {
        val alias = args.namespace + ".native-sync-wrap-v1"
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        val present = store.getKey(alias, null)
        if (present != null) return present as SecretKey
        require(create)
        val directory = File(activity.noBackupFilesDir, "mdreader-sync-vault-v1")
        val files = directory.list() ?: emptyArray()
        require(files.size <= 256 && files.none { it.startsWith(args.namespace + ".") })
        return KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore").apply {
            init(KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM).setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256).setRandomizedEncryptionRequired(true).build())
        }.generateKey()
    }
    @Synchronized fun read(invoke: Invoke) {
        try {
            val args = invoke.parseArgs(VaultRequest::class.java); validate(args)
            val file = file(args)
            if (!file.baseFile.exists() && !File(file.baseFile.path + ".bak").exists()) { invoke.resolve(JSObject()); return }
            val encrypted = file.openRead().use { input ->
                val opened = Os.fstat(input.fd)
                require(OsConstants.S_ISREG(opened.st_mode) && opened.st_nlink == 1L && opened.st_uid == android.os.Process.myUid())
                require(opened.st_mode and 63 == 0 && input.channel.size() in 29..(32*1024))
                val bound = ByteArray(32*1024+1); var size = 0
                while (size < bound.size) {
                    val read = input.read(bound, size, bound.size-size)
                    if (read < 0) break
                    size += read
                }
                require(size <= 32*1024)
                bound.copyOf(size).also { bound.fill(0) }
            }
            val cipher = Cipher.getInstance("AES/GCM/NoPadding")
            cipher.init(Cipher.DECRYPT_MODE, key(args, false), GCMParameterSpec(128, encrypted.copyOfRange(0,12)))
            cipher.updateAAD((args.namespace + ":" + args.reference).toByteArray(Charsets.UTF_8))
            val plaintext = cipher.doFinal(encrypted, 12, encrypted.size-12)
            try { require(plaintext.size <= 16384); invoke.resolve(JSObject().put("bytes", Base64.encodeToString(plaintext, Base64.NO_WRAP))) }
            finally { plaintext.fill(0); encrypted.fill(0) }
        } catch (_: Exception) { invoke.reject("custody_unavailable") }
    }
    @Synchronized fun write(invoke: Invoke) {
        var plaintext: ByteArray? = null
        try {
            val args = invoke.parseArgs(VaultRequest::class.java); validate(args)
            require((args.bytes?.length ?: 65537) <= 65536)
            val decoded = Base64.decode(requireNotNull(args.bytes), Base64.NO_WRAP)
            plaintext = decoded; require(decoded.size <= 16384)
            val file = file(args)
            // Existing ciphertext with a lost key must not be overwritten with
            // a newly generated key. Rust also verifies exact custody CAS first.
            val hasRecord = file.baseFile.exists() || File(file.baseFile.path + ".bak").exists()
            val cipher = Cipher.getInstance("AES/GCM/NoPadding")
            cipher.init(Cipher.ENCRYPT_MODE, key(args, !hasRecord))
            cipher.updateAAD((args.namespace + ":" + args.reference).toByteArray(Charsets.UTF_8))
            val encrypted = cipher.iv + cipher.doFinal(decoded)
            val output = file.startWrite()
            try {
                val opened = Os.fstat(output.fd)
                require(OsConstants.S_ISREG(opened.st_mode) && opened.st_nlink == 1L && opened.st_uid == android.os.Process.myUid())
                Os.fchmod(output.fd, 384); output.write(encrypted); file.finishWrite(output)
            }
            catch (error: Exception) { file.failWrite(output); throw error }
            finally { encrypted.fill(0) }
            invoke.resolve()
        } catch (_: Exception) { invoke.reject("custody_unavailable") }
        finally { plaintext?.fill(0) }
    }
}
