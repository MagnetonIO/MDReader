# Cold Android selected-intent acceptance

This is a coordinator-run test for an explicitly named, owned MDReader emulator.
The host script is authored but has not been run. It does not reset storage or
use receiver instrumentation. Install the matching debug app and test APK with
the coordinated build first; the test APK needs `adb install -t`.

Use repository-pinned Node 24 and an `adb` on `PATH`. Supply an exact emulator
serial and an AVD name beginning with `MDReader`. Both are checked against the
running emulator; physical devices are refused. Use a new metadata-receipt path
inside the MDReader-owned acceptance cache. Existing receipts are never replaced.

Run from the repository root, substituting the actual owned emulator values:

```sh
node src-tauri/gen/android/app/src/androidTest/host/cold-intent-acceptance.mjs \
  --opt-in-owned-empty-emulator true \
  --serial "$MDREADER_EMULATOR" \
  --avd "$MDREADER_OWNED_AVD" \
  --action VIEW \
  --output "$MDREADER_COLD_VIEW_RECEIPT"
```

The script first launches MDReader normally to inspect **only counts** through
its ordinary reader API. Existing or interrupted documents cause refusal before
any fixture is created or sent. It then stops MDReader through Android, confirms
its PID is absent, prepares fixed synthetic fixtures in the test APK's separate
UID, and checks PID absence again immediately before the sender's `ACTION_VIEW`.
The sender grants read access only to that explicit synthetic content URI.

The receiving process is not instrumented or replaced for inspection. The host
connects to the debug WebView socket belonging to its exact PID and uses normal
`mobile_reader_list`, `mobile_reader_read`, and `mobile_reader_search` commands.
It verifies one published receipt, exact BOM/CRLF UTF-8 bytes and the real index,
then verifies the actual visible Reader marker. The body hash is computed inside
the receiver WebView; only synthetic receipt/hash metadata leaves it. No body,
private filename, credentials, callback URL or provider-account data is logged.
The SDK sender independently rechecks its unchanged original bytes.

After publication and UI reading have completed, the host closes the probe,
force-stops MDReader, records PID absence, and launches the normal app again
without replaying the selection. The new PID, same exact receipt/body hash,
index result and absence of duplicate imports are checked repeatedly. The final
receipt records installed app/test APK hashes, UIDs, PIDs, timestamps and the
stable exact-copy hash. Nothing calls the native queue or import command.

For a separate cold share-in case, run the same command with `--action SEND`
and a new receipt path on another **empty owned** emulator/library. The script
cannot reuse VIEW's populated library and will preserve/refuse it. Only the
coordinator may arrange another disposable empty emulator or explicitly retire
their synthetic test data; this script performs no deletion or reset. SEND
must publish into the actual Library. The test then opens that shipped Library
row through its normal button and verifies the visible Reader before restart.

An optional `--run UUIDv4` chooses a known synthetic fixture run ID. By default
a fresh UUID is generated. Each sender operation also has a new public operation
ID, so an earlier logcat success cannot satisfy a later inspection. The sender
logs only this public test identity, operation, UID pair and completion code.
Originals and published copies are retained after the test for inspection.

If WebView debugging, an expected SDK component or the normal reader API is
unavailable, the script fails without an alternate import path. A pass covers
the selected emulator/debug APK only. It does not establish release WebView
debugging, physical-device grants, public cloud-provider access, share-sheet
visual acceptance, first-install delivery, power-loss durability or other Android
versions. The baseline launch initializes the empty library, but the receiver
is truly process-cold for the selected intent. Keep screenshots and build/source
provenance alongside the metadata receipt at the coordinated acceptance gate.
