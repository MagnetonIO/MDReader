# Android scroll-derived reading position acceptance

This coordinator-run case preserves the retained public profile on
`MDReaderReaderAcceptanceAPI34`, serial `emulator-5554`. It refuses other AVDs,
physical devices, another shipping APK, unknown documents or pending imports.
The baseline must contain only the already accepted `external-send.md`, ID
`b5fd90eb-2c6a-430a-ab75-9476d50ed182`, revision
`39627de4f71d547964423be57005a2a169e9f9e386bd54fcb9751113391962d0`.
Its original, receipt, pin and saved line must remain unchanged.
Native revision identity is BLAKE3; independent raw-byte SHA-256 is verified
separately against the retained cold-SEND receipt, not treated as a revision.
Do not clear app data, uninstall the receiver, replace its APK merely to retry, or repeat the import to recover a
failed check. A failed case retains its added copy for inspection and refuses
another fresh-import run against that changed Library. The bounded resume mode
below accepts only the exact retained run and two receipts; it never repeats
preparation, grant delivery or import.

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

Every UI or lifecycle action has a synchronous, fsync'd metadata checkpoint in
`<output>.stages.ndjson` before input, then a success record after observation.
The ledger records native saved line, Library row line when visible, requested
line, visible source block/line/relative offset, scroll geometry, exact receipt
and raw-byte hashes, PID and pinned APK identities. No Markdown bodies, tokens
or expressions enter it. A failure saves an explicit failed result and final
read-only observation, preserving the pending action and all prior checkpoints.
The ledger refuses existing paths and has a 256-record / 1 MiB bound.

The host preserves the exact baseline before sending anything. It verifies the
new original and exact published bytes, indexed search and actual virtualized
Reader DOM. Android `input swipe` gestures move the reader beyond source line
100. Read-only list/read inspection must show the exact scroll-derived line.
Before every Android swipe and system Back, the host requires the exact receiver
MainActivity to be the sole resumed Activity; background CDP access alone cannot
authorize system input.
The shipped Back/Library controls close and reopen the document; the top source
block must match that saved line. No direct import, queue, position, pin or
database mutation is available in this host script.

After returning to Library, normal Android Back must background the task while
retaining the same process; an ordinary launch must warm resume with the exact
saved line. Android 12 and later [background root launcher Activities on
Back](https://developer.android.com/about/versions/12/behavior-changes-all#activity-lifecycle).
The test then verifies the final exact two public documents and saved line,
AVD/hardware, both APK hashes, receiver PID and sole foreground MainActivity.
As a separately authorized OS restart gate, it force-stops only this receiver on
the owned synthetic emulator. It records confirmed PID absence, relaunches the
ordinary Activity, requires a distinct PID, and checks the same receipt, bytes,
search, line and rendered location. The receipt labels this as OS force-stop,
not normal application quit. Delayed
observations must show exactly two documents without duplicate replay. The
separate sender finally re-reads and hashes the original; both imported copies
and the exact fixture directory remain retained. Disconnecting the WebView probe
removes only its own ADB forward. Do not use `cleanup-reading` until the preserved
original is no longer needed and the coordinator explicitly retires that exact
synthetic run.

The first run against `7cd4ba0` failed at its initial Library reopen. Both
copies and the external original were retained. That older host did not record
the exact saved line before reopening, so the later observed line 101 is not
proof of its original requested line. The warm/cold lifecycle phases did not
run. The new stage recorder and bounded resume have not been runtime accepted.

For the coordinator's next justified attempt, resume only this already imported
public run, with the same baseline above and long-document receipt:

- Run `af50b48c-5a1e-420b-b4c8-5183a9f43ca4`
- ID `339c9e4a-26ce-4016-94fb-668e07e69d1e`
- Source `45881be69f22e8365e9cbb3f0a731644fdb8b87ff647ceddf3dc27a9b5e43d05`
- Revision `b5c35ebe0212efba5e3ea87305edd8826dc77fc23b565d893d728e2b0fee5e5d`
- Raw-byte SHA-256 `10e85c875d2cf7f98bb4394ec294a2593af1ace5d113c1b38531f6fbfacf0423`

```sh
node src-tauri/gen/android/app/src/androidTest/host/reading-position-acceptance.mjs \
  --serial emulator-5554 --avd MDReaderReaderAcceptanceAPI34 \
  --opt-in-retained-synthetic-profile true \
  --resume-existing-reading-fixture true \
  --run af50b48c-5a1e-420b-b4c8-5183a9f43ca4 \
  --output /ABS/OWNED/ACCEPTANCE/new-resume-receipt.json
```

This mode refuses missing, extra or changed receipts before any UI input or body
read of an unknown document. It records the current line independently, advances
by actual gestures to a new observed line, then checks reopen/warm/cold behavior.
It does not label the previous failed gate as successful or its current line as
historical evidence. No app install or fixture cleanup is part of this host.

The original default remains the existing `7cd4ba0` debug APK on the owned API 34 emulator
(APK SHA-256 `8758ea712a2a6dd460bb910333313eef5108d56bf22b9089b5fd57fb42841dd7`).
It is not acceptance of the current release source, a physical device, or other
Android versions. The debug WebView is used for read-only observation and normal
UI control activation; gestures and the process lifecycle come from Android.

## Correctly linked package, retained profile only

The measured `d3ea491` resume against the original APK failed after preserving
line 205 through Back and the Library row: reopening saved/rendered line 203.
Its exact two bodies, receipts, index and baseline line 1 remained intact; warm
and cold phases were not reached. This remains an observed product regression.

The subsequent `b28d487` package compiled but could not start because its build
controller set `CARGO_NET_OFFLINE=true`. That disables ort-sys's normal pinned
runtime resolver even with the runtime archive cached. Android refused the
unresolved native ONNX symbol before reader startup. That package is now refused
by the host allowlist. The original APK was restored by normal `install -r`, with
read-only checks proving the same UID and two exact indexed documents, pins and
current saved lines 1/67. No reset, uninstall, reimport or position mutation ran.
The later line 67 does not replace the historical measured 205→203 failure.

The clean whole-source `c367d56d406c01cc9b36dbfd74bc7c9bdc45060f` package contains
shared scroll restoration, compact mobile reading chrome, shared appearance and
the fail-fast mobile runtime guard. Its two-job warm build finished normally
with all runtime-disabling flags unset. Artifact inspection verified exactly one
arm64 native entry, a single exact source identity, a defined local `OrtGetApiBase`
function and the same debug certificate/bundle ID as the retained original.
APK SHA-256: `b2f07994410221d424564aad34211eba556b18f759adb45222ded193bfbb688f`.
Build logs and immutable package provenance are under
`android-runtime-corrected-controller/c367d56d406c01cc9b36dbfd74bc7c9bdc45060f/`
in the MDReader acceptance cache. Build and symbol checks are not runtime acceptance.

The c367 package started and passed actual scrolling, saved line 169 and first
UI reopen. Root Android Back then closed its task; a new task opened in the same
PID, followed by Tao process exit and a native EGL destroyed-mutex abort. The
same-PID/off-foreground observation did not prove activity retention. That run
remains failed under `android-c367-retained-reading/`; c367 is now refused by
the host allowlist. A separate guarded post-crash ordinary launch verified the
same two exact indexed copies, pins and saved lines 1/169 without reimport.

## Root Back repair package, retained profile only

Reviewed root Back repair `50e19a5` is in app source
`2dcc8fcdcd3ab2cf77a1ef8ac497c5b3302fc742`, alongside the combined reader and
search-coverage source. The warm two-job Android build compiled Kotlin and
finished normally with runtime-disabling flags unset. The package has one exact
source identity, the defined ONNX entrypoint and the unchanged debug certificate.
APK SHA-256: `e8a7904b8bbe6e2f2440f269e260afb5a166ba7b592c8fb9da1c23bb85933acb`.
Artifact provenance remains under `android-runtime-corrected-controller/2dcc8fcdcd3ab2cf77a1ef8ac497c5b3302fc742/`.
Runtime acceptance is pending. The host now requires the same nonfinishing root
Activity token and task ID in STOPPED background and RESUMED foreground states,
in addition to the existing PID, exact receipt/body and saved-position fences.

The host admits only the original baseline and this exact repaired source/APK pair.
The corrected package requires the existing two-document resume, fixed public
run, receipt/body identity, sole foreground receiver, owned AVD, PID and APK
checks. It accepts no arbitrary hash override and installs no APK itself.

Before a coordinator-owned normal `adb install -r` upgrade, verify the two
synthetic copies through read-only native calls, old installed APK, owned AVD,
exact UID and matching certificate. Preserve the actual old APK. After upgrade,
verify the new installed hash and unchanged native records before any UI input.
Stop on a mismatch; preserve data without resetting or reimporting.

```sh
node src-tauri/gen/android/app/src/androidTest/host/reading-position-acceptance.mjs \
  --serial emulator-5554 --avd MDReaderReaderAcceptanceAPI34 \
  --opt-in-retained-synthetic-profile true \
  --resume-existing-reading-fixture true \
  --app-source-commit 2dcc8fcdcd3ab2cf77a1ef8ac497c5b3302fc742 \
  --run af50b48c-5a1e-420b-b4c8-5183a9f43ca4 \
  --output /ABS/OWNED/ACCEPTANCE/new-corrected-resume-receipt.json
```

The stage ledger records the actual app and host source plus both APK hashes.
Any eventual pass is limited to the owned API-34 debug package and retained
synthetic run; it cannot establish iOS, physical-device, release signing,
provider OAuth or complete redesign acceptance.
