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
incomplete imports. The existing native asset owner retains its separate limits
and version history; its document reads delegate to this shared authority.

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
transitions before sync can apply. Asset-owner extraction, key bootstrap/custody
and product account integration remain separate work.

## Focused development checks

Run from the native development worktree using the coordinator's sole check slot
and compatible existing target cache. No model or installed profile is needed.

```sh
CARGO_BUILD_JOBS=2 cargo test --locked -p mdreader-core --lib device:: -- --test-threads=1
CARGO_BUILD_JOBS=2 cargo test --locked -p mdreader-core --test local_admission
CARGO_BUILD_JOBS=2 cargo clippy --locked -p mdreader-core --all-targets -- -D warnings
```

At an integration checkpoint, compile the native app once and run the focused
asset regressions plus the binding generator from that artifact. These checks do
not substitute for packaged upgrade/device acceptance or a real sync roundtrip.
