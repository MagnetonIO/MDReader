package dev.mdreader.app

import android.view.View
import android.view.ViewGroup
import android.webkit.WebView
import androidx.test.core.app.ActivityScenario
import org.json.JSONObject
import org.json.JSONTokener
import java.util.UUID
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/** Inspect the real app using its ordinary Tauri IPC. No incoming queue calls,
 * native mocks, alternate document writer or production debug hook are used. */
internal class NativeReaderBridge(private val scenario: ActivityScenario<MainActivity>) {
    private fun webView(view: View): WebView? {
        if (view is WebView) return view
        if (view is ViewGroup) for (index in 0 until view.childCount) {
            webView(view.getChildAt(index))?.let { return it }
        }
        return null
    }
    fun evaluate(expression: String): Any? {
        val returned = CountDownLatch(1)
        var value: String? = null
        scenario.onActivity { activity ->
            val web = webView(activity.window.decorView)
            if (web == null) { returned.countDown(); return@onActivity }
            web.evaluateJavascript(expression) { result -> value = result; returned.countDown() }
        }
        check(returned.await(10, TimeUnit.SECONDS)) { "App WebView evaluation timed out" }
        val parsed = JSONTokener(value ?: "null").nextValue()
        return if (parsed === JSONObject.NULL) null else parsed
    }
    fun ready() {
        await("The actual Tauri WebView bridge did not become ready", 60_000) {
            evaluate("typeof window.__TAURI_INTERNALS__?.invoke === 'function'") == true
        }
    }
    fun call(command: String, args: JSONObject = JSONObject()): Any? {
        // Test reads and ordinary reading-state commands only. Ingress must
        // originate from OS intents and be drained by the shipped frontend.
        require(command in setOf("mobile_reader_list", "mobile_reader_read", "mobile_reader_search", "mobile_reader_pin"))
        val slot = "__mdreader_acceptance_" + UUID.randomUUID().toString().replace("-", "")
        val key = JSONObject.quote(slot)
        evaluate("""(() => {
            window[$key] = { state: 'pending' };
            window.__TAURI_INTERNALS__.invoke(${JSONObject.quote(command)}, $args)
              .then(value => window[$key] = { state: 'ok', value })
              .catch(error => window[$key] = { state: 'error', error: String(error) });
            return true;
        })()""")
        try {
            var reply: JSONObject? = null
            await("Native reader command did not complete: $command", 30_000) {
                val text = evaluate("JSON.stringify(window[$key] ?? { state: 'missing' })") as? String
                reply = text?.let { JSONObject(it) }
                reply?.optString("state") in setOf("ok", "error")
            }
            val result = checkNotNull(reply)
            check(result.getString("state") == "ok") { "Native reader command failed: $command (${result.optString("error")})" }
            return result.opt("value").let { if (it === JSONObject.NULL) null else it }
        } finally { evaluate("delete window[$key]") }
    }
    fun incomingFinishedWithError(): Boolean = evaluate("""(() => {
        const incoming = document.querySelector('[aria-label="Incoming Markdown"]');
        return !!incoming?.querySelector('[role="alert"]') &&
          !Array.from(incoming.querySelectorAll('[role="status"]')).some(node => node.textContent.includes('Importing selected'));
    })()""") == true
    companion object {
        fun await(message: String, timeout: Long = 60_000, condition: () -> Boolean) {
            val deadline = System.nanoTime() + TimeUnit.MILLISECONDS.toNanos(timeout)
            var last: Exception? = null
            do {
                try { if (condition()) return } catch (cause: Exception) { last = cause }
                Thread.sleep(150)
            } while (System.nanoTime() < deadline)
            throw AssertionError(message, last)
        }
    }
}
