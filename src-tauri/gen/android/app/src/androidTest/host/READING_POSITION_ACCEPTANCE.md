# Android scroll-derived reading position acceptance

This coordinator-run case preserves the retained public profile on
`MDReaderReaderAcceptanceAPI34`, serial `emulator-5554`. It refuses other AVDs,
physical devices, another shipping APK, unknown documents or pending imports.
The baseline must contain only the already accepted `external-send.md`, ID
`b5fd90eb-2c6a-430a-ab75-9476d50ed182`, revision
`39627de4f71d547964423be57005a2a169e9f9e386bd54fcb9751113391962d0`.
Its original, receipt, pin and saved line must remain unchanged. Do not clear app
data, uninstall the receiver, replace its APK or repeat the import to recover a
failed check. A failed case retains its added copy for inspection and refuses
another run against that changed Library.

The new test-APK fixture is one allowlisted `external-reading-position.md`, with
420 fixed sections, over 800 renderer blocks, UTF-8 BOM and CRLF, capped at
64 KiB. Original fixtures retain their 1 KiB caps, names, marker indices and
preparation/inspection behavior. The new `prepare-reading`, `reading` and
`inspect-reading` sender operations use a fresh UUID directory and only this
fixture. The ordinary separate-UID `ACTION_VIEW` grants just its content URI.
No shipping permission or production component changes.

At the combined gate, reuse the already generated, warm Android project after
the reviewed source is integrated. Build only the test APK, offline, without
invoking Rust or assembling/reinstalling the shipping APK:

```sh
./gradlew --offline --no-daemon --max-workers=2 -Dorg.gradle.parallel=false \
  :app:assembleUniversalDebugAndroidTest \
  -x :app:rustBuildUniversalDebug -x :app:rustBuildArm64Debug \
  -x :app:rustBuildArmDebug -x :app:rustBuildX86Debug \
  -x :app:rustBuildX86_64Debug
```

Use the existing compatible Gradle/SDK cache; do not initialize another Tauri
project, download dependencies or launch a native build. Confirm the task graph
and retained app artifact identity before packaging. Install only the test APK
with the compatible SDK's `adb -s emulator-5554 install -r -t ...`. Do not run
receiver instrumentation: it would alter the process boundary under test.
The host also requires the already used Node runtime with global `WebSocket`.

With the accepted shipping app already running, run from the reviewed checkout:

```sh
node src-tauri/gen/android/app/src/androidTest/host/reading-position-acceptance.mjs \
  --serial emulator-5554 --avd MDReaderReaderAcceptanceAPI34 \
  --opt-in-retained-synthetic-profile true \
  --output /ABS/OWNED/ACCEPTANCE/new-reading-position-receipt.json
```

The host preserves the exact baseline before sending anything. It verifies the
new original and exact published bytes, indexed search and actual virtualized
Reader DOM. Android `input swipe` gestures move the reader beyond source line
100. Read-only list/read inspection must show the exact scroll-derived line.
The shipped Back/Library controls close and reopen the document; the top source
block must match that saved line. No direct import, queue, position, pin or
database mutation is available in this host script.

After returning to Library, normal Android Back must end the Activity/process.
If it does not, the test stops without a force-stop fallback. It records confirmed
PID absence, relaunches the ordinary Activity, requires a distinct PID, and
checks the same receipt, bytes, search, line and rendered location. Delayed
observations must show exactly two documents without duplicate replay. The
separate sender finally re-reads and hashes the original; both imported copies
and the exact fixture directory remain retained. Disconnecting the WebView probe
removes only its own ADB forward. Do not use `cleanup-reading` until the preserved
original is no longer needed and the coordinator explicitly retires that exact
synthetic run.

This source is authored, not runtime accepted. A successful metadata receipt
would establish only the existing `7cd4ba0` debug APK on the owned API 34 emulator
(APK SHA-256 `8758ea712a2a6dd460bb910333313eef5108d56bf22b9089b5fd57fb42841dd7`).
It is not acceptance of the current release source, a physical device, or other
Android versions. The debug WebView is used for read-only observation and normal
UI control activation; gestures and the process lifecycle come from Android.
