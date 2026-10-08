package dev.mdreader.transfer

import android.app.Activity
import android.content.ClipData
import android.content.Intent
import android.system.Os
import android.system.OsConstants
import android.webkit.WebView
import androidx.activity.result.ActivityResult
import androidx.appcompat.app.AppCompatActivity
import androidx.core.content.FileProvider
import app.tauri.annotation.ActivityCallback
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Channel
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.io.File
import java.io.FileInputStream
import java.io.FileOutputStream
import java.security.MessageDigest
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean

@InvokeArg
class CopyRequest {
    lateinit var path: String
    lateinit var name: String
    lateinit var mime: String
    lateinit var sha256: String
    var bytes: Long = 0
    lateinit var action: String
}
@InvokeArg
class WatchLinks { lateinit var handler: Channel }

@TauriPlugin
class TransferPlugin(private val activity: Activity): Plugin(activity) {
    private val worker = Executors.newSingleThreadExecutor()
    private val busy = AtomicBoolean(false)
    private var pendingCopy: CopyRequest? = null
    private var listener: Channel? = null
    private val links = ArrayDeque<String>()

    override fun load(webView: WebView) { receive(activity.intent) }
    override fun onNewIntent(intent: Intent) { receive(intent) }
    private fun receive(intent: Intent?) {
        if (intent?.action != Intent.ACTION_VIEW) return
        val url = intent.dataString ?: return
        if (url.length > 16384 || !url.startsWith("mdreader://read?") || url.any { it.code < 32 }) return
        synchronized(links) {
            val current = listener
            if (current != null) current.send(JSObject().put("url", url))
            else {
                if (links.size == 8) links.removeFirst()
                links.addLast(url)
            }
        }
    }
    @Command
    fun watchLinks(invoke: Invoke) {
        val args = invoke.parseArgs(WatchLinks::class.java)
        synchronized(links) {
            listener = args.handler
            while (links.isNotEmpty()) args.handler.send(JSObject().put("url", links.removeFirst()))
        }
        invoke.resolve()
    }
    private fun verified(args: CopyRequest): File {
        require(args.bytes in 0..(20L * 1024 * 1024))
        require(args.action == "export" || args.action == "share")
        require(args.name.isNotEmpty() && args.name.length <= 160 && !args.name.contains('/') && !args.name.contains('\\'))
        require(args.mime == "application/zip" || args.mime == "text/markdown")
        val root = File(activity.cacheDir, "mdreader-transfers-v1").canonicalFile
        val file = File(args.path)
        require(file.absoluteFile == file.canonicalFile && file.canonicalPath.startsWith(root.path + "/"))
        val stat = Os.lstat(file.path)
        require(OsConstants.S_ISREG(stat.st_mode) && stat.st_nlink == 1L && stat.st_size == args.bytes)
        val digest = MessageDigest.getInstance("SHA-256")
        FileInputStream(file).use { input ->
            val buffer = ByteArray(65536); var total = 0L
            while (true) {
                val count = input.read(buffer)
                if (count < 0) break
                total += count; require(total <= args.bytes)
                digest.update(buffer, 0, count)
            }
            require(total == args.bytes)
        }
        require(digest.digest().joinToString("") { "%02x".format(it) } == args.sha256)
        return file
    }
    @Command
    fun presentCopy(invoke: Invoke) {
        if (!busy.compareAndSet(false, true)) { invoke.reject("transfer_busy"); return }
        val args = try { invoke.parseArgs(CopyRequest::class.java) }
        catch (_: Exception) { busy.set(false); invoke.reject("transfer_unavailable"); return }
        worker.execute {
            try {
                val file = verified(args)
                activity.runOnUiThread {
                    try {
                        if (args.action == "export") {
                            pendingCopy = args
                            val intent = Intent(Intent.ACTION_CREATE_DOCUMENT).apply {
                                addCategory(Intent.CATEGORY_OPENABLE); type = args.mime
                                putExtra(Intent.EXTRA_TITLE, args.name)
                            }
                            startActivityForResult(invoke, intent, "exportResult")
                        } else {
                            val uri = FileProvider.getUriForFile(activity, activity.packageName + ".mdreader.transfer", file)
                            val intent = Intent(Intent.ACTION_SEND).apply {
                                type = args.mime; putExtra(Intent.EXTRA_STREAM, uri)
                                clipData = ClipData.newRawUri("Selected reader copy", uri)
                                addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                            }
                            activity.startActivity(Intent.createChooser(intent, "Share reader copy"))
                            // Starting the chooser is not a delivery receipt. The
                            // private copy stays available for its recipient.
                            busy.set(false); invoke.resolve(JSObject().put("state", "presented"))
                        }
                    } catch (_: Exception) { pendingCopy = null; busy.set(false); invoke.reject("transfer_unconfirmed") }
                }
            } catch (_: Exception) { busy.set(false); invoke.reject("transfer_unavailable") }
        }
    }
    @ActivityCallback
    fun exportResult(invoke: Invoke, result: ActivityResult) {
        val args = pendingCopy; pendingCopy = null
        if (result.resultCode == Activity.RESULT_CANCELED) {
            busy.set(false); invoke.resolve(JSObject().put("state", "cancelled")); return
        }
        val uri = result.data?.data
        if (result.resultCode != Activity.RESULT_OK || uri == null || args == null || uri.scheme != "content") {
            busy.set(false); invoke.reject("transfer_unconfirmed"); return
        }
        worker.execute {
            try {
                val file = verified(args)
                activity.contentResolver.openFileDescriptor(uri, "w")?.use { descriptor ->
                    FileInputStream(file).use { input ->
                        FileOutputStream(descriptor.fileDescriptor).use { output ->
                            input.copyTo(output, 65536); output.flush()
                            // Document providers can reject sync; never report
                            // durable export after such a failure.
                            descriptor.fileDescriptor.sync()
                        }
                    }
                } ?: throw IllegalStateException()
                invoke.resolve(JSObject().put("state", "exported"))
            } catch (_: Exception) { invoke.reject("transfer_unconfirmed") }
            finally { busy.set(false) }
        }
    }
    override fun onDestroy(activity: AppCompatActivity) {
        listener = null; worker.shutdown()
    }
}
