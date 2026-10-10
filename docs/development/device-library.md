# Portable device library owner

`mdreader-core::device` owns explicitly imported Markdown, import recovery, pins
and exact-revision line positions. `src-tauri/src/mobile_reader.rs` keeps native
directory selection, blocking-worker scheduling and generated transport DTOs.
The adapters borrow the existing `ReaderStore`; they open no second writer.

The extraction preserves `device_documents_v1`, `device_import_intents_v1`, IDs,
source bindings, retained directory identities and original files in place:
`device-markdown-v1/<UUID>/document.md`. It creates no parallel document library.
Reservations consume quota before files are created. Synced bytes precede the
transaction that publishes source grant, document row and completed intent.
Rebuildable indexing follows durable publication. Disabled source identities
cannot become enabled through recovery or repeated admission.

Production modules separate documents/state, imports, recovery, types, errors
and schema/migrations. Errors crossing the native boundary are fixed typed
diagnostics, including a useful quota error, without raw SQL, paths or content.
Limits remain 1 MiB per Markdown file, 1,000 documents and 64 MiB including
incomplete imports. Portable assets retain their separate limits and version
history; their document reads delegate to this shared authority.

## Mobile search

The Search screen offers Text, Text + meaning and Meaning. Semantic search is
off in a new library. Download & enable records explicit consent in the private
library's `reader_device_search_v1` namespace, prepares the BGE English model and
queues imported documents on the existing embedded service owner. Imports and
rebuilds queue that same scheduler. Turning it off denies new model loads and
inference; cached model bytes and text search remain available. A running native
operation may finish. OS suspension separately closes new owner admission.

One query vector serves the full device library. Retrieval groups stay below
the core scope bound, with global ordering before reciprocal-rank fusion and
exact live document receipts before returning results. No SQLite writer lock
spans model loading or query inference. The UI shows model readiness and the
number of fully embedded documents; meaning-only queries do not substitute text
hits while the model is unavailable.

For an explicit host acceptance using an existing public model cache:

```sh
cargo run --locked -p mdreader-service --features local-embed,sync-native \
  --example embedded_semantic -- /ABS/PUBLIC_BGE_CACHE
```

This uses disposable synthetic documents, verifies a paraphrase with no text
matches, and checks that disabling semantic search preserves text retrieval.
It does not establish physical-device inference or lifecycle acceptance.
Native iOS builds require iOS 15.1 or newer for the linked ONNX runtime. The
launcher pins the compiler even when Xcode runs outside the checkout. Tauri's
mobile environment filter does not forward `ORT_LIB_PATH`; its normal build
uses ort-sys's pinned download/cache resolver. `CARGO_NET_OFFLINE=true` disables
that resolver even for cached runtime archives, so do not set that variable for
the Tauri mobile build. Direct Cargo offline builds can use an explicit matching
`ORT_LIB_PATH`. Never reuse a desktop archive for an iOS target.

## Android root-task lifetime

The shipped `MainActivity` installs a lifecycle-bound AndroidX Back callback
when its real WebView is created. At the task root with no WebView history,
Back moves the task to the background. This also covers Open with, Share and
explicit-component entry, whose original Intent may have no launcher category.
WebView history and non-root Activity navigation delegate through the existing
Tauri callback chain. The callback neither finishes the Activity nor resets,
reopens or replaces the native reader owner or its library.

Android's automatic root-launcher background behavior is not proof that every
entry Intent retains the task. See the official [task/Back behavior](https://developer.android.com/guide/components/activities/tasks-and-back-stack#back-tap-behavior)
and [AndroidX lifecycle-bound callbacks](https://developer.android.com/guide/navigation/navigation-custom-back).
The locked Tauri runtime removes a destroyed final window before emitting a
no-code `ExitRequested`; vendored Tao then calls `std::process::exit`. Preventing
that event alone would retain an event loop whose window had already gone.
There is no blanket exit prevention or dependency/runtime patch here.

The maintained Android reading-position acceptance requires the same nonfinishing
root Activity token and task ID to become `STOPPED` after Back, then `RESUMED`
after ordinary launch. It separately retains process identity, exact bodies,
document receipts, index membership, pins and saved lines. A lingering PID with
zero Activities, or a replacement task/Activity, cannot certify warm resume.
The bounded dump-parser cases test this admission; actual Back/lifecycle behavior
still requires the coordinator's exact packaged app on the owned emulator.

## Portable selected assets

`mdreader-core::device::assets`, enabled by `device-assets`, owns selected PNG/JPEG
copies, manifests, reservations, validation and bounded read sessions. The optional
feature keeps the image decoder out of the default core and headless service.
Tauri retains command DTOs, native directory selection and blocking workers; it
maps typed core diagnostics at that transport boundary. There is one implementation
and one database writer, without new tables or relocated copies.

An asset store and runtime belong to one live `ReaderStore` and retained private
directory. Reopening an owner requires fresh instances, even at the same database
path. Cached reuse, publication and snapshot installation revalidate that binding,
current asset namespace version, directory identity and exact document authority.
Prepared values cannot cross owner/root boundaries, and read permits cannot cross
runtimes. Inventory/Forget require asset authority but remain available without
restoring revoked document authority. Existing quotas, upload replay, fsync and
manifest-generation fencing are preserved.

Prepare under owner access, perform decoding/hash verification outside its mutex,
then revalidate before publishing or installing. Reads remain limited to two
4 MiB snapshots, uploads to one validation at a time, chunks to 64 KiB and session
lifetime to 15 seconds. Explicit Close releases a snapshot without acquiring the
database or asset directory, including after directory replacement. File reads
independently enforce the 4 MiB bound before allocating, even if old table
constraints are absent.

Strict adoption validation for malformed unpublished asset prototypes remains
follow-up debt; this extraction preserves the existing asset version/adoption
rules. A native Chunk rejected before acquiring its asset store retains its
session until Close or expiry; the client must close in its cleanup path. Core
Chunk invalidation releases its session immediately.

## Schema and preservation

The independent `device_document_schema_v1` marker records version 1. The known
previous unmarked producer layout is validated before adoption; populated
documents, pins, positions, enabled/disabled sources and reserved/published
intents are retained. Exact documents-only partial initialization can acquire an
empty intents table, without inventing import history or relocation authority.
Unknown layouts or future versions fail closed without resetting records.
Namespace creation/adoption and marker publication share one transaction.
Global durable-library version 4 and disposable-index version 5 are unchanged.

The owner retains the existing conservative recovery behavior: a damaged enabled
published directory can fail the whole library listing, including access to
pending cleanup. Final empty-directory cleanup also retains a pathname race.
These are follow-up issues; extraction does not claim to resolve either.

## Transaction-aware source publication

`publish_local_source` accepts the caller's existing SQLite transaction. It
validates retained root identity and private-provider-cache separation before and
after trusted metadata publication, preserving revoked sources. It neither
starts nor commits a transaction, writes files, opens another database nor indexes.
The caller must propagate failure and roll back the whole outer transaction.
Success remains provisional until that transaction commits. Durable filesystem
intents precede it; indexing follows commit. Multiple-root batches need final
validation of every retained root after all callbacks.

This enables future owner publication inside `SyncStore::mark_applied`; incoming
projection is still absent. A native raw-BLAKE3 content revision is distinct from
a wire UUID revision and needs an explicit scoped association. Replacement,
dirty conflicts, tombstones and unsupported block/pixel positions require owner
transitions before sync can apply. Applying document plus asset publication in
one supplied transaction, key bootstrap/custody and product account integration
remain separate work. Portable asset publication still commits its own transaction;
it is not yet the incoming sync transaction seam.

## Focused development checks

Run from the native development worktree using the coordinator's sole check slot
and compatible existing target cache. No model or installed profile is needed.

```sh
CARGO_BUILD_JOBS=2 cargo test --locked -p mdreader-core --lib device:: -- --test-threads=1
CARGO_BUILD_JOBS=2 cargo test --locked -p mdreader-core --features device-assets --lib device::assets:: -- --test-threads=1
CARGO_BUILD_JOBS=2 cargo test --locked -p mdreader-core --test local_admission
CARGO_BUILD_JOBS=2 cargo clippy --locked -p mdreader-core --all-targets -- -D warnings
```

At an integration checkpoint, compile the native app once and run the focused
native adapter regression plus the binding generator from that artifact. These checks do
not substitute for packaged upgrade/device acceptance or a real sync roundtrip.
