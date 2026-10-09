package dev.mdreader.app

import android.app.Activity
import android.content.ComponentName
import android.content.Intent
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.os.ResultReceiver
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.util.UUID
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/** Separate opt-in acceptance. The test APK's Activity owns the provider files
 * and sends OS intents from its independent UID. Instrumentation only observes
 * ordinary reader IPC/DOM and pins a published copy; it never calls ingress. */
@RunWith(AndroidJUnit4::class)
class ExternalIncomingMarkdownAcceptanceTest {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val target = instrumentation.targetContext
    private val arguments = InstrumentationRegistry.getArguments()
    private fun guard() {
        assertEquals("Use only the coordinator-owned synthetic emulator", "true", arguments.getString("mdreaderExternalSyntheticAcceptance"))
        assertTrue("Real devices are refused", ExternalFixtureProtocol.emulator())
        assertEquals(ExternalFixtureProtocol.targetPackage, target.packageName)
        assertEquals(ExternalFixtureProtocol.senderPackage, instrumentation.context.packageName)
    }
    private fun sender(run: String, operation: String): Bundle {
        val done = CountDownLatch(1)
        var code = Activity.RESULT_CANCELED
        var returned: Bundle? = null
        val receiver = object : ResultReceiver(Handler(Looper.getMainLooper())) {
            override fun onReceiveResult(resultCode: Int, resultData: Bundle?) {
                code = resultCode; returned = resultData; done.countDown()
            }
        }
        instrumentation.context.startActivity(Intent().setComponent(ComponentName(
            ExternalFixtureProtocol.senderPackage, ExternalFixtureSenderActivity::class.java.name))
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            .putExtra("syntheticOwnedEmulator", true).putExtra("run", run)
            .putExtra("operation", operation).putExtra("reply", receiver))
        assertTrue("Independent sender did not return", done.await(15, TimeUnit.SECONDS))
        assertEquals("Independent synthetic sender refused this operation", Activity.RESULT_OK, code)
        val reply = checkNotNull(returned)
        assertNotEquals("Provider fixture and receiver must have different UIDs", reply.getInt("targetUid"), reply.getInt("senderUid"))
        assertEquals(target.applicationInfo.uid, reply.getInt("targetUid"))
        val hashes = JSONObject(checkNotNull(reply.getString("hashes")))
        ExternalFixtureProtocol.names.forEach { name ->
            assertEquals("External provider original changed", ExternalFixtureProtocol.digest(ExternalFixtureProtocol.bytes(run, name)), hashes.getString(name))
        }
        return reply
    }
    private fun documents(bridge: NativeReaderBridge): List<JSONObject> {
        val library = bridge.call("mobile_reader_list") as JSONObject
        assertEquals("No interrupted imports should remain", 0, library.getJSONArray("pending").length())
        val rows = library.getJSONArray("documents")
        return (0 until rows.length()).map(rows::getJSONObject)
    }
    private fun verify(bridge: NativeReaderBridge, run: String, names: List<String>): List<JSONObject> {
        var docs = emptyList<JSONObject>()
        NativeReaderBridge.await("External OS selections did not publish exactly once") {
            docs = documents(bridge)
            assertTrue("An external selection was imported more than once", docs.size <= names.size)
            docs.size == names.size && names.all { name -> docs.count { it.getString("name") == name } == 1 }
        }
        for (doc in docs) {
            assertEquals(4, UUID.fromString(doc.getString("id")).version())
            assertTrue(doc.getString("source_id").matches(Regex("[a-f0-9]{64}")))
            assertTrue(doc.getString("revision").matches(Regex("[a-f0-9]{64}")))
            val read = read(bridge, doc)
            for (field in listOf("id", "source_id", "revision", "name", "bytes", "pinned"))
                assertEquals("Reading changed its exact receipt: $field", doc.get(field), read.getJSONObject("document").get(field))
            assertArrayEquals("External URI admission changed BOM/CRLF bytes", ExternalFixtureProtocol.bytes(run, doc.getString("name")), read.getString("markdown").toByteArray(Charsets.UTF_8))
            NativeReaderBridge.await("External publication was not searchable in the real index") {
                val hits = bridge.call("mobile_reader_search", JSONObject().put("query", ExternalFixtureProtocol.marker(run, doc.getString("name")))) as JSONArray
                (0 until hits.length()).any { index ->
                    val hit = hits.getJSONObject(index).getJSONObject("document")
                    hit.getString("id") == doc.getString("id") && hit.getString("source_id") == doc.getString("source_id") &&
                        hit.getString("revision") == doc.getString("revision") && hit.getBoolean("indexed")
                }
            }
        }
        return docs
    }
    private fun read(bridge: NativeReaderBridge, doc: JSONObject): JSONObject = bridge.call("mobile_reader_read",
        JSONObject().put("id", doc.getString("id")).put("revision", doc.getString("revision"))) as JSONObject
    /** Canonical receipt + actual body digest; no wall clock or index timing.
     * The host carries this fixed SHA-256 to a separate post-restart run. */
    private fun receiptHash(bridge: NativeReaderBridge, docs: List<JSONObject>): String {
        val fields = docs.sortedBy { it.getString("name") }.flatMap { doc ->
            listOf("id", "source_id", "revision", "name", "bytes", "pinned").map { doc.get(it).toString() } +
                ExternalFixtureProtocol.digest(read(bridge, doc).getString("markdown").toByteArray(Charsets.UTF_8))
        }
        return ExternalFixtureProtocol.digest(fields.joinToString("") { "${it.toByteArray(Charsets.UTF_8).size}:$it" }.toByteArray(Charsets.UTF_8))
    }
    private fun freshErrorState(scenario: ActivityScenario<MainActivity>, bridge: NativeReaderBridge) {
        scenario.recreate(); bridge.ready()
        NativeReaderBridge.await("Recreated app did not finish its ordinary pending-selection check") {
            bridge.evaluate("""(() => {
                const panel = document.querySelector('[aria-label="Incoming Markdown"]');
                return !panel?.querySelector('[role="alert"]') && !Array.from(panel?.querySelectorAll('[role="status"]') ?? [])
                    .some(node => node.textContent.includes('Importing selected'));
            })()""") == true
        }
    }
    @Test
    fun externalUidSelectionsPublishAndEmitRestartProof() {
        guard()
        assertNull("Use the separate restart method for a retained Library", arguments.getString("mdreaderExpectedReceiptHash"))
        val run = ExternalFixtureProtocol.token(arguments.getString("mdreaderFixtureRunId") ?: UUID.randomUUID().toString())
        // Keep the native Activity alive. Closing its last Activity can terminate
        // the native event loop and instrumentation itself on this host.
        val scenario = ActivityScenario.launch<MainActivity>(Intent(target, MainActivity::class.java))
        val bridge = NativeReaderBridge(scenario); bridge.ready()
        assertTrue("Existing Library is preserved; use a fresh owned emulator", documents(bridge).isEmpty())
        sender(run, "prepare")
        sender(run, "view")
        val first = verify(bridge, run, ExternalFixtureProtocol.accepted.take(1)).single()
        NativeReaderBridge.await("Actual ACTION_VIEW did not open the imported Markdown in Reader") {
            bridge.evaluate("document.querySelector('[aria-label=\"Reader\"]')?.textContent.includes(${JSONObject.quote(ExternalFixtureProtocol.marker(run, first.getString("name")))}) ?? false") == true
        }
        bridge.call("mobile_reader_pin", JSONObject().put("id", first.getString("id")).put("pinned", true))
        val pinnedHash = receiptHash(bridge, verify(bridge, run, ExternalFixtureProtocol.accepted.take(1)))
        scenario.recreate(); bridge.ready()
        repeat(5) {
            assertEquals("Recreation replayed or changed the exact external copy", pinnedHash,
                receiptHash(bridge, verify(bridge, run, ExternalFixtureProtocol.accepted.take(1))))
            Thread.sleep(200)
        }
        sender(run, "send"); verify(bridge, run, ExternalFixtureProtocol.accepted.take(2))
        sender(run, "multiple")
        val firstFour = ExternalFixtureProtocol.accepted.take(4)
        val firstFourHash = receiptHash(bridge, verify(bridge, run, firstFour))
        freshErrorState(scenario, bridge)
        assertEquals(firstFourHash, receiptHash(bridge, verify(bridge, run, firstFour)))
        sender(run, "revoked")
        NativeReaderBridge.await("The independent provider's denied URI was not refused in the UI") { bridge.incomingFinishedWithError() }
        assertEquals(firstFourHash, receiptHash(bridge, verify(bridge, run, firstFour)))
        sender(run, "inspect")
        // A real successful selection must recover after the denied URI and
        // clear its old refusal. The next assertion cannot pass on a stale alert.
        sender(run, "recover")
        val completeHash = receiptHash(bridge, verify(bridge, run, ExternalFixtureProtocol.accepted))
        NativeReaderBridge.await("A valid selection did not clear the previous refusal") { !bridge.incomingFinishedWithError() }
        freshErrorState(scenario, bridge)
        sender(run, "invalid")
        NativeReaderBridge.await("Invalid/non-Markdown external selections were not refused in the UI") { bridge.incomingFinishedWithError() }
        assertEquals(completeHash, receiptHash(bridge, verify(bridge, run, ExternalFixtureProtocol.accepted)))
        sender(run, "inspect")
        // Preserve only these synthetic provider originals and published copies
        // for the coordinator's explicit OS process restart and second method.
        instrumentation.sendStatus(0, Bundle().apply {
            putString("mdreaderFixtureRunId", run)
            putString("mdreaderReceiptHash", completeHash)
            putString("mdreaderProofStage", "external-uid-before-process-restart")
        })
    }
    @Test
    fun retainedReceiptsMatchAfterExplicitOsProcessRestart() {
        guard()
        val run = ExternalFixtureProtocol.token(checkNotNull(arguments.getString("mdreaderFixtureRunId")))
        val expected = checkNotNull(arguments.getString("mdreaderExpectedReceiptHash"))
        require(expected.matches(Regex("[a-f0-9]{64}")))
        assertEquals("Coordinator must explicitly stop the app between instrumentation launches", "true", arguments.getString("mdreaderProcessRestarted"))
        val scenario = ActivityScenario.launch<MainActivity>(Intent(target, MainActivity::class.java))
        val bridge = NativeReaderBridge(scenario); bridge.ready()
        val restored = verify(bridge, run, ExternalFixtureProtocol.accepted)
        assertEquals("Process restart changed exact copies, receipt identity or pins", expected, receiptHash(bridge, restored))
        assertTrue(restored.single { it.getString("name") == ExternalFixtureProtocol.accepted.first() }.getBoolean("pinned"))
        repeat(5) {
            assertEquals("Delayed startup replay changed retained receipts", expected, receiptHash(bridge, verify(bridge, run, ExternalFixtureProtocol.accepted)))
            Thread.sleep(200)
        }
        sender(run, "inspect")
        if (arguments.getString("mdreaderCleanupExternalFixtures") == "true") sender(run, "cleanup")
        instrumentation.sendStatus(0, Bundle().apply {
            putString("mdreaderReceiptHash", expected)
            putString("mdreaderProofStage", "external-uid-after-process-restart")
        })
    }
}
