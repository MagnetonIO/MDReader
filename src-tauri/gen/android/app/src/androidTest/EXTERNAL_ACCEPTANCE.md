# External Android selected-file acceptance

This opt-in harness is separate from `IncomingMarkdownAcceptanceTest`. It runs
only on a coordinator-owned disposable emulator with an empty MDReader Library.
It refuses real devices and existing libraries; it never resets app data. Install
the matching app and androidTest APK using the coordinated Android build. The
test APK requires `adb install -t` and adds no shipping component or permission.
Select one method explicitly; running the entire class would run its retained
library phase without the required host restart arguments.

Use the owned emulator's serial in every command. First run:

```sh
adb -s "$MDREADER_EMULATOR" shell am instrument -w \
  -e mdreaderExternalSyntheticAcceptance true \
  -e class dev.mdreader.app.ExternalIncomingMarkdownAcceptanceTest#externalUidSelectionsPublishAndEmitRestartProof \
  dev.mdreader.app.test/androidx.test.runner.AndroidJUnitRunner
```

The first phase launches a sender Activity belonging to the **test APK's UID**.
It proves this differs from the receiving app UID, creates eight fixed synthetic
fixtures beneath its own narrow SDK-only cache file provider, and sends real Android
`ACTION_VIEW`, `ACTION_SEND` and `ACTION_SEND_MULTIPLE` intents. It checks exact
BOM/CRLF bytes, unique native receipts, actual Reader text, indexed search and
pin retention after Activity recreation. A selected fixture is explicitly
granted and revoked before delivery; this and non-Markdown/invalid UTF-8
selections must finish with shipped UI refusal and unchanged originals. A fifth
valid recovery selection separates the two refusals, proving that normal
publication resumes and preventing a stale alert from satisfying the next check.

Save the emitted `mdreaderFixtureRunId` and `mdreaderReceiptHash` from the
instrumentation result. The hash includes stable receipt fields, the pin and
SHA-256 of each **actual native read**. It excludes index timing and timestamps.
Keep the imported copies and external synthetic cache fixtures for phase two.

After phase one succeeds, record the target PID if it remains alive, explicitly
stop the target process, and confirm it is gone. Do not clear storage:

```sh
adb -s "$MDREADER_EMULATOR" shell pidof dev.mdreader.app
adb -s "$MDREADER_EMULATOR" shell am force-stop dev.mdreader.app
adb -s "$MDREADER_EMULATOR" shell pidof dev.mdreader.app
```

Record these host results alongside both test results. If the app already ended
when instrumentation finished, record that observation too. Then substitute the
two emitted values in a **new** instrumentation launch:

```sh
adb -s "$MDREADER_EMULATOR" shell am instrument -w \
  -e mdreaderExternalSyntheticAcceptance true \
  -e mdreaderProcessRestarted true \
  -e mdreaderFixtureRunId "$MDREADER_FIXTURE_RUN_ID" \
  -e mdreaderExpectedReceiptHash "$MDREADER_RECEIPT_HASH" \
  -e mdreaderCleanupExternalFixtures true \
  -e class dev.mdreader.app.ExternalIncomingMarkdownAcceptanceTest#retainedReceiptsMatchAfterExplicitOsProcessRestart \
  dev.mdreader.app.test/androidx.test.runner.AndroidJUnitRunner
```

Phase two performs no imports or pin changes. It reads the five retained exact
copies, verifies search and provider originals, compares the same fixed hash,
and observes for delayed duplicate replay. Optional cleanup removes only the
eight exact test-APK fixtures belonging to the supplied run; imported copies
remain for inspection. Omit cleanup if further provider-original inspection is
needed. Uninstalling the test APK later retires its isolated synthetic cache.

Limits: these tests have been authored, not run. The restart flag is an explicit
coordinator assertion, not proof of an OS stop by itself; retain the host PID and
force-stop evidence. The harness exercises warm OS intent forwarding and
Activity recreation. It does not claim cold intent delivery, public provider
account access, share-sheet visual acceptance or grants on another Android
version. The external components use only SDK/Java classes because the test APK
does not package target-provided AndroidX/Kotlin dependencies. The three Java
helpers pass a direct compile against the Android 36 SDK; APK packaging and
actual separate-UID execution still require the coordinated gate.
It uses ordinary read/search/pin IPC only; ingress must come from the
OS and the shipped UI drain. No direct queue/import commands or debug hooks are
available to these tests.
