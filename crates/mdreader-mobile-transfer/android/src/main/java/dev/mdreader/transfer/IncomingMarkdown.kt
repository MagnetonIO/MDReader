package dev.mdreader.transfer

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.CancellationSignal
import android.provider.OpenableColumns
import android.system.Os
import android.system.ErrnoException
import android.system.OsConstants
import android.util.AtomicFile
import android.util.Base64
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import org.json.JSONArray
import org.json.JSONObject
import java.io.ByteArrayOutputStream
import java.io.Closeable
import java.io.File
import java.io.FileInputStream
import java.io.FileOutputStream
import java.io.InputStream
import java.nio.ByteBuffer
import java.nio.charset.CodingErrorAction
import java.security.MessageDigest
import java.util.UUID
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.Executors
import java.util.concurrent.ThreadPoolExecutor
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference

/** Only explicit OS content-URI selections enter this private durable queue.
 * One exact item is retained until the Rust document owner acknowledges it.
 * No URI, provider permission, body or filesystem locator reaches the renderer. */
interface IncomingMarkdownHost { val incomingMarkdown: IncomingMarkdown }

/** One instance belongs to the Android Application, never to an Activity. */
class IncomingMarkdown(context: Context) {
    private val context = context.applicationContext
    private val observerLock = Any()
    private var observer: (() -> Unit)? = null
    fun observe(callback: () -> Unit): Closeable {
        synchronized(observerLock) { observer = callback }
        return Closeable { synchronized(observerLock) { if (observer === callback) observer = null } }
    }
    private fun notifyReady() {
        val callback = synchronized(observerLock) { observer }
        try { callback?.invoke() } catch (_: Exception) { /* A detached Activity is not queue failure. */ }
    }
    private val worker = ThreadPoolExecutor(1, 1, 0, TimeUnit.MILLISECONDS, ArrayBlockingQueue<Runnable>(4))
    private val timer = Executors.newSingleThreadScheduledExecutor()
    @Volatile private var overflow = false
    private val maxFile = 1024 * 1024
    private val maxTotal = 8L * 1024 * 1024
    private val maxFiles = 20
    private fun root(): File {
        val value = File(context.noBackupFilesDir, "mdreader-incoming-v1")
        require(value.mkdir() || value.isDirectory)
        require(value.absoluteFile == value.canonicalFile)
        val stat = Os.lstat(value.path)
        require(OsConstants.S_ISDIR(stat.st_mode) && stat.st_uid == android.os.Process.myUid())
        Os.chmod(value.path, 448)
        return value
    }
    private fun manifest(root: File): AtomicFile {
        val file = File(root, "queue.json")
        for (candidate in arrayOf(file, File(file.path + ".bak"), File(file.path + ".new"))) {
            if (!candidate.exists()) continue
            require(candidate.absoluteFile == candidate.canonicalFile)
            val stat = Os.lstat(candidate.path)
            require(OsConstants.S_ISREG(stat.st_mode) && stat.st_uid == android.os.Process.myUid() && stat.st_nlink == 1L && stat.st_mode and 63 == 0 && stat.st_size <= 32768)
        }
        return AtomicFile(file)
    }
    private fun index(root: File): JSONObject {
        val file = manifest(root)
        val value = if (file.baseFile.exists() || File(file.baseFile.path + ".bak").exists()) JSONObject(String(file.openRead().use { bounded(it, 32768) }, Charsets.UTF_8))
            else JSONObject().put("version", 1).put("items", JSONArray())
        require(value.getInt("version") == 1 && value.getJSONArray("items").length() <= maxFiles)
        return value
    }
    private fun persist(root: File, value: JSONObject) {
        if (overflow) { value.put("overflow", true); overflow = false }
        val bytes = value.toString().toByteArray(Charsets.UTF_8)
        require(bytes.size <= 32768)
        val file = manifest(root)
        val output = file.startWrite()
        try {
            Os.fchmod(output.fd, 384); output.write(bytes); output.fd.sync(); file.finishWrite(output)
            require(file.openRead().use { bounded(it, 32768) }.contentEquals(bytes))
            syncDirectory(root)
        }
        catch (error: Exception) { file.failWrite(output); throw error }
    }
    private fun bounded(input: InputStream, limit: Int): ByteArray {
        val output = ByteArrayOutputStream(); val buffer = ByteArray(65536)
        while (true) {
            val count = input.read(buffer); if (count < 0) break
            require(output.size() + count <= limit); output.write(buffer, 0, count)
        }
        return output.toByteArray()
    }
    private fun syncDirectory(root: File) {
        // O_DIRECTORY is not exposed by the public Android OsConstants API.
        // Open without following a replacement link, then verify the held FD is
        // the same owned directory before syncing its entries.
        val before = Os.lstat(root.path)
        val fd = Os.open(root.path, OsConstants.O_RDONLY or OsConstants.O_NOFOLLOW or OsConstants.O_CLOEXEC, 0)
        try {
            val held = Os.fstat(fd)
            require(OsConstants.S_ISDIR(held.st_mode) && held.st_uid == android.os.Process.myUid()
                && held.st_dev == before.st_dev && held.st_ino == before.st_ino)
            Os.fsync(fd)
        } finally { Os.close(fd) }
    }
    private fun token(value: String): String { require(UUID.fromString(value).version() == 4); require(UUID.fromString(value).toString() == value); return value }
    private fun body(root: File, id: String): File {
        val file = File(root, token(id) + ".md")
        require(file.absoluteFile == file.canonicalFile)
        val stat = Os.lstat(file.path)
        require(OsConstants.S_ISREG(stat.st_mode) && stat.st_uid == android.os.Process.myUid() && stat.st_nlink == 1L && stat.st_mode and 63 == 0 && stat.st_size <= maxFile)
        return file
    }
    private fun digest(bytes: ByteArray): String = MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it) }
    private fun issue(root: File, index: JSONObject, code: String) {
        val items = index.getJSONArray("items")
        if (items.length() < maxFiles) items.put(JSONObject().put("id", UUID.randomUUID().toString()).put("failure", code))
        else index.put("overflow", true)
        persist(root, index)
    }
    @Suppress("DEPRECATION")
    private fun selected(intent: Intent): List<Uri> {
        val uris = LinkedHashSet<Uri>()
        if (intent.action == Intent.ACTION_VIEW) intent.data?.let { uris.add(it) }
        else {
            if (intent.action == Intent.ACTION_SEND) intent.getParcelableExtra<Uri>(Intent.EXTRA_STREAM)?.let { uris.add(it) }
            if (intent.action == Intent.ACTION_SEND_MULTIPLE) {
                val streams = intent.getParcelableArrayListExtra<Uri>(Intent.EXTRA_STREAM).orEmpty()
                require(streams.size <= maxFiles); uris.addAll(streams)
            }
            intent.clipData?.let { clip -> require(clip.itemCount <= maxFiles); for (position in 0 until minOf(clip.itemCount, maxFiles + 1)) clip.getItemAt(position).uri?.let { uris.add(it) } }
        }
        require(uris.isNotEmpty() && uris.size <= maxFiles)
        require(uris.all { it.scheme == "content" })
        return uris.toList()
    }
    fun receive(intent: Intent?): Boolean {
        if (intent == null || intent.action !in arrayOf(Intent.ACTION_VIEW, Intent.ACTION_SEND, Intent.ACTION_SEND_MULTIPLE)) return false
        if (intent.action == Intent.ACTION_VIEW && intent.data?.scheme != "content") return false
        // Opening the app again from Android history must not re-import its old intent.
        if (intent.flags and Intent.FLAG_ACTIVITY_LAUNCHED_FROM_HISTORY != 0 || intent.getBooleanExtra("dev.mdreader.incoming.handled", false)) return true
        intent.putExtra("dev.mdreader.incoming.handled", true)
        try { worker.execute { stage(intent) } }
        catch (_: Exception) { overflow = true; notifyReady() }
        return true
    }
    private fun stage(intent: Intent) {
        try {
            val root = root(); val index = index(root); reconcileAcknowledged(root, index)
            val uris = try { selected(intent) } catch (_: Exception) { issue(root, index, "invalid_selection"); notifyReady(); return }
            val deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(60)
            for (uri in uris) {
                if (index.getJSONArray("items").length() >= maxFiles) { index.put("overflow", true); persist(root, index); break }
                try {
                    val remaining = deadline - System.nanoTime(); require(remaining > 0)
                    val signal = CancellationSignal()
                    // This timeout owns only this URI's stream. A late callback
                    // can never close the next URI after Activity recreation.
                    val streamLease = AtomicReference<Closeable?>()
                    val timeout = timer.schedule({ signal.cancel(); try { streamLease.getAndSet(null)?.close() } catch (_: Exception) {} }, minOf(remaining, TimeUnit.SECONDS.toNanos(15)), TimeUnit.NANOSECONDS)
                    val name: String
                    val bytes: ByteArray
                    try {
                        name = context.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null, signal)?.use { cursor ->
                            if (cursor.moveToFirst()) cursor.getString(0) else null
                        } ?: if (intent.type in arrayOf("text/markdown", "text/x-markdown")) "shared-markdown.md" else throw IllegalArgumentException()
                        require(name.isNotEmpty() && name.toByteArray(Charsets.UTF_8).size <= 240 && name.none { Character.isISOControl(it) || it == '/' || it == '\\' })
                        require(name.lowercase().endsWith(".md") || name.lowercase().endsWith(".markdown"))
                        val descriptor = context.contentResolver.openFileDescriptor(uri, "r", signal) ?: throw IllegalStateException()
                        val input = android.os.ParcelFileDescriptor.AutoCloseInputStream(descriptor); streamLease.set(input)
                        bytes = input.use { stream ->
                            val output = ByteArrayOutputStream(); val buffer = ByteArray(65536)
                            while (true) {
                                signal.throwIfCanceled(); val count = stream.read(buffer); if (count < 0) break
                                require(output.size() + count <= maxFile); output.write(buffer, 0, count)
                            }
                            output.toByteArray()
                        }
                        // Validation only: do not normalize BOM, newlines or bytes.
                        Charsets.UTF_8.newDecoder().onMalformedInput(CodingErrorAction.REPORT).onUnmappableCharacter(CodingErrorAction.REPORT).decode(ByteBuffer.wrap(bytes))
                    } finally { timeout.cancel(false); try { streamLease.getAndSet(null)?.close() } catch (_: Exception) {} }
                    val retained = root.listFiles()?.filter { it.name.matches(Regex("[a-f0-9-]{36}\\.md")) }.orEmpty()
                    require(retained.size < maxFiles && retained.sumOf { body(root, it.name.removeSuffix(".md")).length() } + bytes.size <= maxTotal)
                    val id = UUID.randomUUID().toString(); val copy = File(root, "$id.md")
                    val row = JSONObject().put("id", id).put("name", name).put("bytes", bytes.size).put("sha256", digest(bytes)).put("open", intent.action == Intent.ACTION_VIEW).put("state", "reserved")
                    // Persist explicit selection before writing its body. An
                    // interruption cannot turn an orphan into inferred consent.
                    index.getJSONArray("items").put(row); persist(root, index)
                    require(copy.createNewFile())
                    FileOutputStream(copy).use { output -> Os.fchmod(output.fd, 384); output.write(bytes); output.fd.sync() }
                    syncDirectory(root)
                    row.put("state", "ready"); persist(root, index)
                } catch (_: Exception) { issue(root, index, "unreadable_selection") }
            }
            notifyReady()
        } catch (_: Exception) { overflow = true; notifyReady() }
    }
    fun notifyPending() { try { worker.execute { try { val value = index(root()); if (value.getJSONArray("items").length() > 0 || value.optBoolean("overflow") || overflow) notifyReady() } catch (_: Exception) { notifyReady() } } } catch (_: Exception) {} }
    fun next(invoke: Invoke) {
        try { worker.execute {
            try {
                val root = root(); val index = index(root); reconcileAcknowledged(root, index)
                if (overflow) persist(root, index)
                val items = index.getJSONArray("items")
                if (items.length() == 0 && index.optBoolean("overflow")) { index.put("overflow", false); issue(root, index, "selection_limit") }
                if (items.length() == 0) { invoke.resolve(JSObject().put("item", null).put("remaining", 0)); return@execute }
                val row = items.getJSONObject(0); token(row.getString("id"))
                if (!row.has("failure") && row.getString("state") == "reserved") {
                    // Recover only a complete exact body selected in this
                    // durable record. Incomplete streams report a safe failure.
                    try {
                        val bytes = FileInputStream(body(root, row.getString("id"))).use { bounded(it, maxFile) }
                        require(bytes.size == row.getInt("bytes") && digest(bytes) == row.getString("sha256"))
                        row.put("state", "ready")
                    } catch (_: Exception) { row.put("failure", "unreadable_selection") }
                    persist(root, index)
                }
                val value = JSObject().put("id", row.getString("id")).put("name", null).put("bytes", 0).put("base64", null).put("failure", row.optString("failure").ifEmpty { null }).put("open", row.optBoolean("open"))
                if (!row.has("failure")) {
                    require(row.getString("state") == "ready")
                    val file = body(root, row.getString("id")); val bytes = FileInputStream(file).use { bounded(it, maxFile) }
                    require(bytes.size <= maxFile && bytes.size == row.getInt("bytes") && digest(bytes) == row.getString("sha256"))
                    value.put("name", row.getString("name")).put("bytes", bytes.size).put("base64", Base64.encodeToString(bytes, Base64.NO_WRAP))
                }
                invoke.resolve(JSObject().put("item", value).put("remaining", items.length() + if (index.optBoolean("overflow")) 1 else 0))
            } catch (_: Exception) { invoke.reject("incoming_unavailable") }
        } } catch (_: Exception) { invoke.reject("incoming_busy") }
    }
    /** Cleanup uses only durable acknowledged rows, never a directory crawl.
     * A crash after unlink leaves the marker; retry syncs the directory again
     * before retiring it. A failed unlink keeps both marker and quota charged. */
    private fun reconcileAcknowledged(root: File, index: JSONObject) {
        val items = index.getJSONArray("items"); val next = JSONArray(); var changed = false
        for (position in 0 until items.length()) {
            val row = items.getJSONObject(position)
            if (row.optString("state") != "acknowledged") { next.put(row); continue }
            val id = token(row.getString("id")); val file = File(root, "$id.md")
            val present = try { Os.lstat(file.path); true }
                catch (error: ErrnoException) { if (error.errno != OsConstants.ENOENT) throw error; false }
            if (present) { body(root, id); require(file.delete()) }
            changed = true
        }
        if (changed) {
            syncDirectory(root)
            index.put("items", next); persist(root, index)
        }
    }
    fun acknowledge(invoke: Invoke, id: String) {
        try { worker.execute {
            try {
                val root = root(); val index = index(root); token(id)
                val items = index.getJSONArray("items")
                for (position in 0 until items.length()) {
                    val row = items.getJSONObject(position)
                    if (row.getString("id") != id) continue
                    // Keep the exact body record until deletion and directory
                    // sync finish. Unknown outcomes never re-import this row.
                    row.put("state", "acknowledged"); persist(root, index); break
                }
                reconcileAcknowledged(root, index)
                invoke.resolve()
            } catch (_: Exception) { invoke.reject("incoming_unavailable") }
        } } catch (_: Exception) { invoke.reject("incoming_busy") }
    }
    // The Application owns these executors for its process lifetime. Activity
    // teardown detaches observation; it cannot leave a second queue writer.
}
