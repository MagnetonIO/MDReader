package dev.mdreader.app

import android.content.ClipData
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import androidx.core.content.FileProvider
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.util.UUID

/** Run only on a coordinator-owned disposable emulator with an empty Library:
 * adb shell am instrument -w -e mdreaderSyntheticAcceptance true
 *   -e class dev.mdreader.app.IncomingMarkdownAcceptanceTest
 *   dev.mdreader.app.test/androidx.test.runner.AndroidJUnitRunner
 * This test preserves the app's imported result for subsequent OS/UI inspection.
 */
@RunWith(AndroidJUnit4::class)
class IncomingMarkdownAcceptanceTest {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val target = instrumentation.targetContext
    private data class Fixture(val file: File, val uri: Uri, val bytes: ByteArray, val marker: String)
    private fun intent(action: String, fixtures: List<Fixture>): Intent {
        require(fixtures.isNotEmpty())
        val result = Intent(action).setPackage(target.packageName)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_GRANT_READ_URI_PERMISSION)
        if (action == Intent.ACTION_VIEW) result.setDataAndType(fixtures.single().uri, "text/markdown")
        else {
            result.type = "text/plain"
            if (action == Intent.ACTION_SEND) result.putExtra(Intent.EXTRA_STREAM, fixtures.single().uri)
            else result.putParcelableArrayListExtra(Intent.EXTRA_STREAM, ArrayList(fixtures.map { it.uri }))
        }
        result.clipData = ClipData.newRawUri("Synthetic Markdown acceptance", fixtures.first().uri).apply {
            fixtures.drop(1).forEach { addItem(ClipData.Item(it.uri)) }
        }
        val resolved = target.packageManager.resolveActivity(result, PackageManager.MATCH_DEFAULT_ONLY)
        assertNotNull("The shipping intent filters did not resolve the selected Markdown", resolved)
        assertEquals(MainActivity::class.java.name, resolved!!.activityInfo.name)
        return result
    }
    private fun fixture(directory: File, name: String, bytes: ByteArray, marker: String = ""): Fixture {
        val file = File(directory, name)
        file.outputStream().use { output -> output.write(bytes); output.flush() }
        return Fixture(file, FileProvider.getUriForFile(target, target.packageName + ".fileprovider", file), bytes, marker)
    }
    private fun markdown(directory: File, name: String): Fixture {
        val marker = "androidingress" + UUID.randomUUID().toString().replace("-", "")
        val bytes = "\uFEFF# Synthetic acceptance\r\n\r\n$marker\r\n".toByteArray(Charsets.UTF_8)
        return fixture(directory, name, bytes, marker)
    }
    private fun documents(bridge: NativeReaderBridge): List<JSONObject> {
        val library = bridge.call("mobile_reader_list") as JSONObject
        assertEquals("No interrupted native copy is expected for valid fixtures", 0, library.getJSONArray("pending").length())
        return library.getJSONArray("documents").let { rows -> (0 until rows.length()).map(rows::getJSONObject) }
    }
    private fun awaitDocuments(bridge: NativeReaderBridge, fixtures: List<Fixture>): List<JSONObject> {
        var result = emptyList<JSONObject>()
        NativeReaderBridge.await("The OS-selected files did not reach the actual native Library") {
            result = documents(bridge)
            assertTrue("An OS selection was imported more than once", result.size <= fixtures.size)
            result.size == fixtures.size && fixtures.all { fixture -> result.count { it.getString("name") == fixture.file.name } == 1 }
        }
        return result
    }
    private fun assertReceipt(expected: JSONObject, actual: JSONObject) {
        for (field in listOf("id", "source_id", "revision", "name", "bytes")) assertEquals("Exact receipt changed: $field", expected.get(field), actual.get(field))
        assertEquals(4, UUID.fromString(actual.getString("id")).version())
        assertTrue(actual.getString("source_id").matches(Regex("[a-f0-9]{64}")))
        assertTrue(actual.getString("revision").matches(Regex("[a-f0-9]{64}")))
    }
    private fun verifyPublished(bridge: NativeReaderBridge, fixtures: List<Fixture>): List<JSONObject> {
        val docs = awaitDocuments(bridge, fixtures)
        for (fixture in fixtures) {
            val doc = docs.single { it.getString("name") == fixture.file.name }
            val read = bridge.call("mobile_reader_read", JSONObject().put("id", doc.getString("id")).put("revision", doc.getString("revision"))) as JSONObject
            assertReceipt(doc, read.getJSONObject("document"))
            assertArrayEquals("Native import changed BOM, CRLF or bytes", fixture.bytes, read.getString("markdown").toByteArray(Charsets.UTF_8))
            assertArrayEquals("The provider's original was modified", fixture.bytes, fixture.file.readBytes())
            NativeReaderBridge.await("Published Markdown was not searchable through its real index") {
                val hits = bridge.call("mobile_reader_search", JSONObject().put("query", fixture.marker)) as JSONArray
                (0 until hits.length()).any { index ->
                    val receipt = hits.getJSONObject(index).getJSONObject("document")
                    receipt.getString("id") == doc.getString("id") && receipt.getString("source_id") == doc.getString("source_id") && receipt.getString("revision") == doc.getString("revision") && receipt.getBoolean("indexed")
                }
            }
        }
        return docs
    }
    @Test
    fun osSelectedMarkdownPublishesExactCopiesAndSurvivesActivityRecreation() {
        assertEquals("Opt in only for the owned synthetic emulator", "true", InstrumentationRegistry.getArguments().getString("mdreaderSyntheticAcceptance"))
        assertTrue("This acceptance test is emulator-only", Build.HARDWARE.contains("ranchu") || Build.HARDWARE.contains("goldfish") || Build.MODEL.contains("Emulator") || Build.MODEL.contains("Android SDK") || Build.PRODUCT.contains("sdk"))
        // Refuse to write into a pre-existing Library rather than reset any data.
        // Keep a single Activity alive: finishing the last Tauri Activity exits
        // its native event loop and therefore the instrumentation process. The
        // coordinator tests process restart separately after the runner exits.
        val scenario = ActivityScenario.launch<MainActivity>(Intent(target, MainActivity::class.java))
        val bridge = NativeReaderBridge(scenario); bridge.ready()
        assertTrue("Use a fresh owned emulator; existing Library is preserved", documents(bridge).isEmpty())
        val directory = File(target.cacheDir, "incoming-acceptance/" + UUID.randomUUID().toString())
        assertTrue(directory.mkdirs())
        val selected = listOf(markdown(directory, "opened.md"), markdown(directory, "shared.md"), markdown(directory, "multiple-one.md"), markdown(directory, "multiple-two.markdown"))
        val invalid = listOf(fixture(directory, "refused.txt", "# Not selected Markdown".toByteArray()), fixture(directory, "invalid-utf8.md", byteArrayOf(0xff.toByte(), 0xfe.toByte())))
        try {
            // Warm ACTION_VIEW uses normal manifest resolution and the actual
            // FileProvider's selected URI; this is not a cold-process test.
            run {
                target.startActivity(intent(Intent.ACTION_VIEW, listOf(selected[0])))
                val original = verifyPublished(bridge, listOf(selected[0])).single()
                NativeReaderBridge.await("Open with did not display the imported Markdown") {
                    bridge.evaluate("document.querySelector('main[aria-label=\"Reader\"]')?.textContent.includes(${JSONObject.quote(selected[0].marker)}) ?? false") == true
                }
                bridge.call("mobile_reader_pin", JSONObject().put("id", original.getString("id")).put("pinned", true))
                scenario.recreate(); bridge.ready()
                val restored = verifyPublished(bridge, listOf(selected[0])).single()
                assertReceipt(original, restored); assertTrue(restored.getBoolean("pinned"))
                // Keep observing after recreation: a replay cannot add a second
                // row while a newly installed WebView drains cold selections.
                repeat(5) { assertReceipt(original, documents(bridge).single()); Thread.sleep(200) }
                target.startActivity(intent(Intent.ACTION_SEND, listOf(selected[1])))
                verifyPublished(bridge, selected.take(2))
                target.startActivity(intent(Intent.ACTION_SEND_MULTIPLE, selected.drop(2)))
                val receipts = verifyPublished(bridge, selected).associateBy { it.getString("name") }
                scenario.recreate(); bridge.ready()
                for (doc in verifyPublished(bridge, selected)) assertReceipt(receipts.getValue(doc.getString("name")), doc)
                // Both non-Markdown and invalid UTF-8 traverse the real OS URI
                // adapter. Wait for the shipped UI's completed refusal state.
                target.startActivity(intent(Intent.ACTION_SEND_MULTIPLE, invalid))
                NativeReaderBridge.await("Invalid selections did not report a completed refusal") { bridge.incomingFinishedWithError() }
                val afterRefusal = documents(bridge)
                assertEquals(selected.size, afterRefusal.size)
                for (doc in afterRefusal) assertReceipt(receipts.getValue(doc.getString("name")), doc)
                invalid.forEach { assertArrayEquals(it.bytes, it.file.readBytes()) }
            }
        } finally {
            // Remove only originals created above in the isolated test cache;
            // imported app documents remain for the coordinator's inspection.
            (selected + invalid).forEach { assertTrue(it.file.delete()) }
            assertTrue(directory.delete())
        }
    }
}
