package dev.mdreader.transfer

import android.app.Activity
import android.content.Intent
import android.net.Uri
import android.os.Handler
import android.os.Looper
import app.tauri.annotation.InvokeArg
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import java.util.UUID

@InvokeArg class AuthenticationRequest { lateinit var attempt: String; lateinit var authorization: String; lateinit var redirect: String; var provider: String? = null }
@InvokeArg class CancelAuthentication { lateinit var attempt: String }
/** System browser only. Callback consumed by native state, never a WebView event. */
class AccountSession(private val activity: Activity) {
    private data class Pending(val id: String, val redirect: Uri, val invoke: Invoke)
    private var pending: Pending? = null
    private val main = Handler(Looper.getMainLooper())
    private var timeout: Runnable? = null
    fun authenticate(invoke: Invoke) {
        val args = try { invoke.parseArgs(AuthenticationRequest::class.java) } catch (_: Exception) { invoke.reject("authentication_unavailable"); return }
        main.post {
            if (pending != null) { invoke.reject("authentication_busy"); return@post }
            try {
                require(UUID.fromString(args.attempt).toString() == args.attempt)
                val expected = when (args.provider) {
                    null -> activity.packageName + ".auth://callback/mdreader-sync"
                    "dropbox" -> activity.packageName + ".auth://callback/dropbox"
                    else -> throw IllegalArgumentException("unsupported")
                }
                require(args.redirect == expected)
                require(args.authorization.length <= 16384)
                val url = Uri.parse(args.authorization)
                require(url.scheme == "https" && url.userInfo == null && url.host != null)
                require(url.getQueryParameters("redirect_uri") == listOf(args.redirect))
                if (args.provider == "dropbox") require(url.host == "www.dropbox.com" && url.path == "/oauth2/authorize")
                pending = Pending(args.attempt, Uri.parse(args.redirect), invoke)
                val expire = Runnable { finish("cancelled") }
                timeout = expire; main.postDelayed(expire, 300000)
                activity.startActivity(Intent(Intent.ACTION_VIEW, url).apply { addCategory(Intent.CATEGORY_BROWSABLE) })
            } catch (_: Exception) { if (pending?.id == args.attempt) finish(null) else invoke.reject("authentication_unavailable") }
        }
    }
    fun receive(intent: Intent?): Boolean {
        val uri = intent?.data ?: return false
        if (uri.scheme != activity.packageName + ".auth") return false
        main.post {
            val current = pending ?: return@post
            if (uri.toString().length > 16384 || uri.scheme != current.redirect.scheme || uri.host != current.redirect.host
                || uri.path != current.redirect.path || uri.port != -1 || uri.userInfo != null || uri.fragment != null) { finish(null); return@post }
            pending = null; timeout?.let { main.removeCallbacks(it) }; timeout = null
            current.invoke.resolve(JSObject().put("state", "completed").put("url", uri.toString()))
        }
        return true
    }
    private fun finish(state: String?) {
        val current = pending; pending = null
        timeout?.let { main.removeCallbacks(it) }; timeout = null
        if (state == null) current?.invoke?.reject("authentication_unconfirmed")
        else current?.invoke?.resolve(JSObject().put("state", state))
    }
    fun cancel(invoke: Invoke) {
        val args = invoke.parseArgs(CancelAuthentication::class.java)
        main.post { if (pending?.id == args.attempt) finish("cancelled"); invoke.resolve() }
    }
    fun close() { main.post { finish(null) } }
}
