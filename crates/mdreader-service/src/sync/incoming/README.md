# Native incoming owner coordinator

`sync-native` opts the service into native sync authority and core image decoding.
Default service/core builds do not acquire the decoder through this module.
This is a service owner API, not renderer RPC or shipping UI acceptance.

Native composition constructs **one** `IncomingCoordinator` per live
`Arc<Mutex<ReaderStore>>`, with explicitly selected document/asset directories and
the trusted setup's **shared remote sync library UUID**. The constructor stores
that immutable remote binding transactionally on the actual owner connection.
It is distinct from the provider registry's local profile UUID. Existing different
bindings, future schemas and malformed namespaces are rejected without reset.
Cross-library adoption/switching needs a separate explicit lifecycle.

Keep the actual `NativeAuthority` alive, call `select(&authority)`, then borrow a
privately constructed `mdreader_sync::incoming::PreparedIncoming` through
`apply(&selection, &prepared)` on a bounded blocking worker. Keep the opaque input
for reconciliation after an uncertain reply. Selection rotates a
fresh UUID even for the same library. `clear_selection` invalidates prior handles.
The handle is opaque, non-cloneable and bound to this exact coordinator/owner.
There is no plaintext constructor, authentication boolean or automatic grant.

An operation try-lock spans reservation, file writes, decoding and publication.
Authority/selection read barriers and the actual owner's try-lock are held only
for short SQLite/root/metadata work. Copying, hashing and pixel decoding run
outside those locks. No network, OS credential calls or model calls occur here.
Reopening the owner requires a fresh coordinator and asset runtime.

Only exact staged, next contiguous **new Content with no parents and no existing
item association** can publish. Other content histories, user state and opaque
tombstones return typed pending reasons with the applied cursor unchanged.
An applied receipt associates separate wire item/revision UUIDs with the fresh
local document/source and raw UTF-8 BLAKE3 revision. Title, source SHA256,
selected asset targets/MIME/SHA256/bytes and materialized asset UUIDs/dimensions
remain exact; body bytes are not rewritten. Core quotas/identity checks apply.

`native_incoming_schema_v1` and ordered migration `001-incoming.sql` independently
guard `native_incoming_items_v1`; they do not change global library/index schema
versions or duplicate document/asset authority. At most 4096 immutable operation
rows are retained, identities at most 16KiB and receipts 32KiB. A reserved row
stores full origin/identity **before** core reservation/files. Same live-origin
retries deduplicate; changed session/selection origins return
`reauthorization_required`. Ordinary local import recovery never activates them.
No automatic cleanup, discard, origin rebinding or history pruning is exposed.
Explicit reauthorization/discard and downgrade/export procedures remain gates;
older clients cannot account for outstanding incoming reservations.

Core publication, item association/receipt and applied cursor share the borrowed
owner transaction. The final check rereads exact staged metadata/cursor and
receipt, revalidates core source/document/assets/root/cache/file evidence, then
checks the real session immediately before commit. Any failure rolls all authority
and cursor writes back. Index work is queued **after durable commit**; queue failure
leaves an applied receipt with `index_queued:false`. Exact replay/reconciliation
repairs enqueue without republishing or reviving a disabled source.

On an uncertain result, retain the verified selected input and call
`reconcile(&current_selection, &prepared)`. A newer valid selection may inspect a
completed old receipt; it cannot rebind a reserved origin. A receipt describes
historical durable application, not current source access or successful indexing.

The focused fixtures exercise real SQLite/filesystem/core publication using the
public encrypted conformance payload, deriving explicitly referenced Markdown and
a valid PNG CRC and re-encrypting the resulting exact resources. The original
crypto vector's unreferenced/structural-only PNG is not natively publishable.
The tests call private storage seams with fixed
test fences, proving orchestration and rollback, **not end-to-end issuer identity**.
Actual `ProjectionPermit`/SessionController tests belong to sync; live OIDC/native
composition, product dispatch, replacement/state/tombstone application, explicit
discard/reauthorization and Windows incoming publication remain separate gates.

## Scoped verification

Using the existing warm target and `CARGO_BUILD_JOBS=2`:

- `cargo test --locked -p mdreader-service --features sync-native --lib sync::incoming::tests -- --nocapture`: five passed, 0.10s execution.
- Temporarily omitting only final `revalidate_publication` produced the expected
  single-test failure at “final core check must see SQL following publication”
  (0.05s). Restoring it returned the same five tests to green.
- `cargo clippy --locked -p mdreader-service --features sync-native --all-targets -- -D warnings`: passed, 7.14s.

These checks cover private-seam storage orchestration. They do not establish
native command/UI wiring, live authentication or complete incoming lifecycle.
