# iOS embedded reader smoke — 2026-10-07

Actual arm64 iPhone 17 simulator, iOS 26.5, Xcode/SDK 27.0. Synthetic
Markdown only; no provider account, private document or physical device used.
The installed macOS baseline was preserved. This is debug simulator evidence.

`pnpm --filter @mdreader/app tauri ios build --debug --target aarch64-sim --no-sign --archive-only --ci`
builds the normal app. Its declared floor and native C objects now agree on
iOS 15.0. Xcode 27 rejected the previous iOS 14 configuration; cached blake3
C objects initially retained the SDK's newer target. The checked-in Cargo
environment gives Cargo-only builds the same floor. During this repair only,
the stale target-specific blake3 cache was cleaned before rebuilding.

The standard iOS file picker imported a 117-byte UTF-8 BOM/CRLF Markdown file
downloaded from a disposable loopback fixture server. The durable copy and
picker copy both matched SHA-256
`5d1b441d5b161e8d938c09fc034bde477d8d509890786513e5019d75cddd49af`.

Two actual host failures were repaired:

- Restoring the reader line with `scrollIntoView` moved ancestor containers and
  hid the toolbar. The reader now scrolls its own container and the mobile
  layout bounds the toolbar, evidence and outline. The updated simulator showed
  readable text, Back/Pin/Evidence/Outline, bounded source evidence and outline.
- Replacing the app changed its data-container UUID. Stored absolute source
  paths failed with `Source authority: cannot-inspect`. Published imports now
  restore only persisted UUID copies whose parent/root directory identities
  still match. The existing source ID, enabled state, bytes, pin and position
  remain authoritative. A real directory-rename/reopen regression first failed
  with the same error, then passed; it also rejects replacement objects and
  preserves revoked sources. All four mobile storage behavior tests pass
  (0.11 s), with no whole-app test run.

After repair, the simulator rendered the original text, acknowledged Pin as
Unpin, returned to a pinned Library row and found the imported SQLite passage
through embedded full-text search. App replacements preserved the same source,
original bytes and index. A subsequent restart with the download server stopped
retained the pin in SQLite. The final fixture contains one indexed document and
one lexical SQLite match. See the [machine-readable receipt](2026-10-07-ios-reader-smoke.json).

The final binary is SHA-256
`f13d32660d7aca558fa9018d7b4c4e99d7892a37e7d3d38b7a7d1f358d45d167`,
237,935,800 bytes, `IOSSIMULATOR`, minOS 15.0, SDK 27.0. The preceding binary
`d1440ea87fcdb7843d9a275efe74f875ca0418eb7da57c03ae40f26f1e2e90ed`
was used for the visible reading/pin/evidence/outline/search checks. The final
binary adds a minimum 16 px search-input font after actual iOS focus zoom was
observed. Its build/install/restart and durable data checks pass; the final
focus/viewport GUI recheck remains open because the computer-use surface returned
`cgWindowNotFound`. No replacement UI automation was used.

The simulator was shut down after the smoke. The disposable VS Code host and
fixture servers were stopped. Physical devices, minimum-iOS runtime coverage,
airplane mode, long-document viewport-position restoration, accessibility,
assets/share/deep links/cloud and signed release acceptance remain open. Debug
linking still reports the inherited large `__eh_frame` warning; this debug
binary includes the legacy app dependencies awaiting Task 7 removal.
