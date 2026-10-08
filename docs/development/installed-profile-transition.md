# Installed-profile preservation and activation gate

Read-only code audit begun at `165e1807f5e7cc4f7f0293b41b4d0a650cea3a26`;
feature refreshed to accepted integration `38b413d77367e26a32b235a3053c2044889a2870`.
This is a proposed next slice, not an installed-profile inventory, implemented
cutover or permission to remove CatDB. No installed data, vault, provider or
private source was read during the audit. The isolated owner is
`feat/reader-profile-transition` / `MDReader-profile-transition`.

## Existing boundaries and blockers

The native startup currently opens `APP_DATA/reader-v1` and also configures the
legacy cloud catalog and schedules cloud sync
([`launch_application`](../../src-tauri/src/lib.rs)). There is no authoritative
active-profile selector or migration coordinator. A new export cannot claim to
have selected a CatDB-free runtime: both owners currently start.

The current offline exporter is a sound preservation component, with a narrower
contract: explicit standalone files, retained descriptors/identities/hashes,
streaming raw copies, immutable read-only queries against copies, bounded member
set/hash verification and private no-replace publication
([`export` / `verify`](../../crates/mdreader-legacy-export/src/lib.rs),
[`Source` / `Stage`](../../crates/mdreader-legacy-export/src/unixfs.rs)). Its
`offlineSnapshots: true` is a caller assertion. It refuses SQLite sidecars and
`reader-v1` paths; it neither establishes writer quiescence nor protects the
complete newer library. Keep those restrictions intact.

The following do **not** establish a profile-wide snapshot barrier:

- [`sessionWriter`](../../ui/src/shell/persistence.ts) serializes sessions, but
  `saveSession` and `saveSettings` swallow errors. Settings do not use that
  serial writer. `boundedPersistenceDrain` and the close handler in
  [`AppShell`](../../ui/src/shell/AppShell.tsx) proceed after two seconds, even
  with writes pending. Memory-only editor buffers need the existing unsaved-edit
  decision before snapshot; a disk export cannot preserve them implicitly.
- [`WorkspaceState` / `revoke_workspace`](../../src-tauri/src/commands.rs) revoke
  a workspace/watcher, but outstanding `Arc<Epoch>` owners can retain an engine
  and proposal connection. Epoch cancellation/stale-result suppression is not
  proof that an already issued write has finished. Review approval spans the
  graph and proposal databases; freeze admission and drain both, rather than
  infer a historical approval from one side.
- [`CloudSyncRuntime`](../../src-tauri/src/cloud_accounts.rs) retains cancellation
  tokens for refresh schedules, but not the spawned task join handles. The
  catalog authority and `CloudDocumentStore` can own separate connections to
  `cloud-library.catdb`. Cancelling future refresh does not join an issued
  refresh/save or resolve an accepted/unknown remote write.
- [`LibraryStore::read`](../../src-tauri/src/library.rs) loads/migrates and retries
  cleanup under `library.lock`. It is not a read-only inventory method.
  `save_to` syncs a temporary file before replacement but does not sync its
  parent directory. The migration snapshot adapter must retain raw bytes without
  invoking registry loaders that normalize, move or delete inputs.
- Desktop [`ReaderBackend`](../../src-tauri/src/reader.rs) starts/reconnects to a
  detached headless service. [`serve_configured`](../../crates/mdreader-service/src/lib.rs)
  holds `reader.lock`, but there is no stop/snapshot/freeze RPC. Closing a window
  does not stop it. External hook processes independently write the capture
  spool. A lifecycle lock unused by existing writers cannot prove quiescence.

## Complete selected inventory

Inventory starts from one explicitly selected, retained app-data directory.
Enumerate bounded directory entries without following symlinks or opening SDKs;
record native path units, object identity, role, presence, byte count and a raw
hash, never private contents in replies/logs. Missing is an explicit inventory
entry. Every discovered entry must be classified; unexplained entries block a
"complete selected profile" receipt. Do not silently skip orphan directories,
unknown files, recovery sidecars or unrecognized schemas.

| Selected artifact | Preservation / coverage |
| --- | --- |
| `library.json` (v9), `settings.json`, `session.json` | Raw exact bytes and unknown fields. Typed interpretations are secondary and may be unavailable. No source grant from restored JSON. |
| `cloud-library.catdb`, sidecars | Whole consistent SQLite snapshot, including mixed SDK catalog/log and all cloud metadata/body/accepted-write repair rows. No source retirement or repair cleanup during preservation. |
| `workspaces/**`, `legacy-v1-path-workspaces/**`, `managed-delete-staging/**` | All selected regular files and relative directory identities, including orphan `catdb.db`, sibling `proposals.db`, unknown members and recoverable cleanup tombstones. Do not discover external corpora from paths in these rows. |
| `mobile-enterprise.json`, `.migrated` | Raw registry artifacts if present; credential references only. No keychain export or reauthorization. |
| Existing `reader-v1/library.sqlite`, sidecars | Protected MDReader-owned state: grants/jobs, provider scopes/generations/admission tombstones/mutation receipts, integration preparation ownership, capture policies, sync outbox/receipts/cursors/incoming history, workload and device document/asset state. Do not import a legacy archive over it. |
| `reader-v1/captures/**` | Durable Markdown, pending queue, receipt/provenance and policies; capture body storage is not an index. Quiesce both drain and external enqueue before its manifest. |
| `reader-v1/provider-cache/**`, `integrations/**`, `native-provider-admissions-v1.json` | Preserve cached bodies for offline availability, installation ownership/settings snapshots and uncertain admission custody references. No credentials, provider calls or replay. |
| `device-markdown-v1/**`, `device-assets-v1/**` | Preserve original private directories, immutable bodies, retained revisions, asset blobs/manifests and pending materializations alongside library metadata. Never relocate/overwrite these as part of export. Native asset root is supplied by `mobile_asset_request` in `mobile_assets_commands.rs`; generic runtime callers may select another explicit root. |
| Explicit selected review archives/staged `reviews.sqlite` | Preserve complete raw archive plus separate staged interpretations if stored outside app-data; registry selection alone is not permission to crawl arbitrary paths. |
| `reader-v1/index.sqlite`, `models/**`, diagnostic logs | Classify explicitly as rebuildable/diagnostic rather than claim durable user-state coverage. Sidecars still need accounted treatment. Preserve unknown rows/files until classified. |
| Lock/endpoint/temp files | Record role/identity and current lifecycle status. Do not copy an endpoint bearer token into migration archives. Do not remove an unknown temporary file merely because its name resembles ours. |
| OS vault, external Markdown/provider originals, agent config outside profile | Out of the archive boundary. Leave untouched for rollback. Record coverage unavailable/external; no secret export, external-source crawling, permission expansion or config uninstall. |

Pointers for these paths: [`db_path_under` / `proposals_path_beside`](../../src-tauri/src/engine.rs),
[`quarantine_v1_workspaces` / `forget_source_with_cleanup_ops`](../../src-tauri/src/library.rs),
[`mobile_reader_import`](../../src-tauri/src/mobile_reader.rs),
[`mobile_asset_request`](../../src-tauri/src/mobile_assets_commands.rs),
[`NativeProviders` admission ledger](../../src-tauri/src/native_providers.rs),
[`capture`](../../crates/mdreader-service/src/capture.rs) and
[`integrations::installer`](../../crates/mdreader-service/src/integrations.rs).
The asset default name and generic selected roots must be checked by the native
adapter; a fixed filename list is insufficient evidence of complete coverage.

Unknown private files require explicit classification, not automatic inclusion or
content scanning for secrets. A lossless claim and a "no secret bytes" claim
cannot both be established for arbitrary opaque unknown input. Mark that gate
unresolved instead of silently dropping the file or claiming a complete export.

## Proposed ownership and API

A new small portable `mdreader-profile` crate owns inventory classifications,
bounded preservation manifests and a lifecycle journal. It has no Tauri, CatDB,
source-grant or OAuth dependency. Modules: `inventory`, `snapshot`, `model`,
`lifecycle` (ordered SQL), `platform` and typed fixed errors. Each production
module stays cohesive. It reuses the existing offline legacy exporter for the
legacy SQLite/JSON interpretation; a separate protected-state manifest accounts
for newer reader files without relaxing the exporter's `reader-v1` exclusion.

A new `src-tauri/src/profile_transition/**` adapter owns the actual owner registry
and single transition lease. Root owns later composition/startup/UI registration;
reader catalog/review files remain with their assigned owner. Snapshot hooks in
legacy/cloud/service owners need explicit coordinated ownership before edits.
No generic boolean `quiesced` or caller-created activation authorization:

```text
inventory(explicit retained root, limits) -> InventoryReview
Coordinator::prepare(selected inventory identity) -> PreparationId + review
Coordinator::snapshot(id, strict renderer flush receipt) -> VerifiedSnapshot
Coordinator::stage(id, explicit destination, converter) -> StagedImportReceipt
Coordinator::activate(id, expected selection generation) -> ActivationReceipt
Coordinator::rollback(id, expected selection generation) -> RollbackReceipt
Coordinator::recover() -> bounded Pending | Active | RollbackPending status
```

Preparation IDs are opaque durable operations, not authority to activate. Review
contains classifications/counts/coverage, not JSON contents or raw database rows.
The coordinator mints a scoped, non-cloneable quiescence capability only after
registered participants deny new effects and all issued effects finish. It is
bound to exact owner/profile identities and one operation generation; timeout,
flush failure, lost participant, profile change or unexplained writer is typed
`NotQuiescent`. Releasing the lease resumes the same generation only when safe.
The UI/runtime bridge must report successful persisted generations for both JSON
writers, without the current error-swallowing/best-effort close behavior.
Capture the final inventory only **after** strict flush succeeds, while the
participant lease continues denying new writes. Hold that same lease through
prepare/stage; a flush after inventory changes file stamps and must invalidate
the operation instead of silently updating its reviewed identity.

Under the barrier, actual owner connections produce consistent SQLite backups
including WAL, using the [SQLite online backup API](https://www.sqlite.org/backup.html).
The API provides a single-database snapshot; the owner barrier supplies the
additional multi-file consistency boundary. Capture files/JSON are copied through retained descriptors;
identities and directory entry sets are checked before/after. Backup/hash/copy
work must not hold reader mutexes for unrelated expensive tasks; a frozen owner
can yield an exclusive snapshot handle or bounded owner backup operation. Do not
open SDKs/migrate original inputs, use `immutable=1` on a live main file, delete
sidecars, or assume independent read transactions create a multi-file instant.
Publish the aggregate manifest only after all bounded members are private,
complete, synced and verified; no-replace directory rename + parent sync. Any
unsupported host remains fail-closed pending private/atomic durability support.

SQLite exposes its main database pathname, not a portable retained file handle.
The crate compares that pathname's current inode to the selected inventory, but
this alone cannot prove an older connection still points at it after pathname
replacement. Native adapters must retain/check database object identity from
owner creation and reject replacement before yielding a snapshot/candidate
connection. Do not open a second connection to turn that missing proof into a
pathname comparison or invoke a legacy SDK to inspect originals.

The current owners do not yet provide these hooks. `ProposalStore::lock` in
[`proposals.rs`](../../crates/mdreader-ingest/src/proposals.rs) and
`CloudDocumentStore::connection` in
[`cloud_documents/store.rs`](../../src-tauri/src/cloud_documents/store.rs) are
private. `EmbeddedCatDb::store/shared_store` exposes the SDK store, whose
`with_conn/with_conn_mut` are crate-private in the currently pinned SDK. Actual
snapshot integration needs narrow bounded owner backup/borrow methods and
identity checks. No external CatDB SDK feature work is planned. Where the SDK
cannot yield its connection, a real shutdown/drain capability must first close
every legacy writer, deny owner restart and retain/check the original database
identity. Only that capability may yield a read-only SQLite backup connection;
account for recovery sidecars and revalidate all retained identities throughout.
This is the narrow fully closed offline fallback, not a caller-supplied offline
flag or active-file copy. Do not claim the trait adapters already have access,
or reopen/migrate the SDK to make the API gap disappear. The other MDReader-owned
connections can gain narrow snapshot hooks in their own assigned slices.

## Activation transaction and restart

Keep original and candidate roots at their original paths/object identities.
Never rename an existing `reader-v1` directory to switch profiles: device roots,
provider caches, capture grants and custody bindings are identity-sensitive.
Existing reader state requires an explicit merge/import plan, not replacement.

One native lifecycle owner controls a small separate **selection** connection;
portable lifecycle code borrows it and opens no library SQLite file. Its
namespaced v1 ordered schema records operation IDs, exact original/candidate
root identities, verified manifest hashes, import receipt hash, expected/current
selection generation and state. It stores no credentials or body contents.
This is profile-selection authority, not a second reader/source/document writer.

The candidate's existing ReaderStore transaction first commits converted user
records, aliases, unavailable/archive records and an immutable import receipt.
No inferred grant, historical review event, present graph membership, current
revision or provider reauthorization. An import receipt alone cannot activate.
After revalidation/quiescence, a **single lifecycle transaction** atomically
records activation receipt and changes the active selector with generation CAS.
The selector references only an already durable verified candidate/import receipt.
This is intentionally two ordered commits, not a claimed cross-file atomic
transaction. Startup must consult the selector before opening either owner and
must not schedule legacy background jobs for a selected reader-only profile.
The app currently lacks that routing; activation remains unavailable until wired.

Crash before candidate receipt: old selector/original remains active; stage is
incomplete. Crash after receipt/before selector: durable unselected candidate,
old still active. Crash after selector/IPC loss: recover exact operation receipt
and selected generation; do not import/activate again. Invalid/missing selected
receipt/root: enter recovery without opening either owner or silently choosing
an older profile. Schema versions newer than supported refuse without reset.

Rollback is an explicit selector transition after quiescing **both** owners and
verifying the untouched original root. It never restores an old snapshot over
source files, reader state, provider bodies or credentials. Any post-activation
writes require a new verified preservation/reconciliation receipt before rollback;
unknown/accepted remote writes remain journaled, never resent or undone by
selection. No automatic GC, deletion, source regrant or indefinite dual writes.

## Small implementation and acceptance sequence

1. Implement bounded explicit inventory, immutable aggregate manifest and borrowed
   lifecycle journal with synthetic owner participants. Public native coordinator
   remains unavailable until real participant registration exists. This proves
   storage/state-machine behavior only, not installed-writer quiescence.
2. Wire real strict JSON flush and joined legacy/service/hook barriers in assigned
   adapter slices, then rehearse preservation against an explicitly selected
   disposable installed-profile clone. Preserve raw original directory identities.
3. Add verified staged user-record import and startup selector routing, then
   explicit activate/rollback UI. Actual installed-profile gate requires the real
   packaged owner/restart path and user-selected profile, not fixture receipts.

Five meaningful fixtures suffice initially: multi-file/unknown/orphan inventory
and exact raw roundtrip; writer/flush timeout with no complete receipt; crash
before/after archive publication and exact reopen; activation CAS/receipt mismatch
with selector unchanged; post-activation write/unknown outcome blocks rollback
until new preservation, while successful rollback preserves both roots bytewise.
Reuse exporter tamper/process-death and archive review interpretation fixtures;
no duplicated branch-by-branch test matrix, broad app suite or private fixtures.

## Required participant hooks and assigned integration order

These are proposed hooks, not existing APIs. The fixed participant list is
`mdreader_profile::REQUIRED_PARTICIPANTS`; the new native adapter lists all eight
as missing and exposes no readiness override. Root coordinates existing-file
patches after the catalog checkpoint; this feature owns only the new crate,
`src-tauri/src/profile_transition/**` and this document until reassigned.

| Participant | Existing owner / concrete hook needed | Completion evidence |
| --- | --- | --- |
| `RendererSession` | Root: `ui/src/shell/persistence.ts`, AppShell and Tauri plugin-store bridge. Deny new session enqueues, cancel debounce/visibility handlers, strictly persist exact latest generation and drain; bind ack to native operation/window epoch. | Save success and native file+parent sync; errors propagate and abort snapshot. Preserve pre-flush raw/unknown records or merge them rather than normalize them away. |
| `RendererSettings` | Root: same files, separate serialized settings writer. Freeze every registered window, not just current session queue. | Exact settings generation saved and synced; no pending callback can issue another write during barrier. |
| `UnsavedEditor` | Root: AppShell existing UnsavedGuard/editor stores. Resolve user save/discard/cancel before taking irreversible transition intent. | No unresolved dirty buffer/in-flight local/provider save; cancellation leaves selection intact. Memory-only buffers are never claimed in disk export. |
| `WorkspaceAndReviews` | Coordinated legacy owner: commands WorkspaceState/Watcher, engine Epoch/proposal handles and overlay mutation dispatch. Admit writes through one operation lease; stop watcher/import scheduling; join issued writes, retain connections for snapshot. | Engine and proposal writers both idle under one retained profile generation, including already-captured old epochs. No library-loader cleanup/discovery during inventory. |
| `LegacyCloudAndEnterprise` | Coordinated native legacy cloud/enterprise owners. Track and join refresh/connect/query/save tasks; fence source/catalog generation before draining. | No newly issued operation; accepted/unknown remote writes durably retained as unknown/repair records, never retried to establish quiescence. Both mixed-catalog and cloud-store connections yield verified backups. |
| `ReaderDaemon` | Coordinated service Runtime/lib admission. New Freeze/Snapshot/Resume scoped RPC with endpoint process/owner epoch and lease. Existing ensure_running must refuse profile activation while frozen. | Owner scheduler/provider/sync/incoming work drained, actual SQLite backup handle and root evidence; daemon remains denied until explicit resume/stop. Closing GUI alone is insufficient. |
| `CaptureProducers` | Coordinated capture/service/native packaged hook path. Capture enqueue and drain participate in the same external-process lease, without revoking user's policy or silently dropping output. | All pending/receipt/Markdown/policy files stable; another hook must return typed busy/unavailable rather than write through freeze. Current queue lock alone is not profile lifecycle authority. |
| `IntegrationAndAdmissionCustody` | Coordinated native providers/integrations/mobile document+asset owners. Deny install/apply, OAuth admission, materialization and custody ledger writes; retain issued operation leases until completion. | No config mutation or partial writer active; preserve current root identities, pending admissions, receipts and existing external settings. Do not delete credentials, repair records or prepared bodies. |

Locked `tauri-plugin-store 2.4.4` has two additional bridge obligations:
`store.rs::StoreInner::save` uses `fs::write`, and `lib.rs` saves registered stores
again on `RunEvent::Exit` even when autosave is disabled. Therefore a renderer
save promise is neither a parent-sync receipt nor a guarantee against exit-time
writes. Native registration must cover plugin store IPC/exit writes as well as
JS queues; do not close the app mid-snapshot and assume the final flush is done.
Diagnostics uses separate `APP_DATA/diagnostics/settings.json`; it is not the UI
settings file. Any diagnostic producer resumed during snapshot also needs an
explicit exclusion/stability policy, not an unexplained changing entry set.

Next implementation order:

1. Complete/check the portable journal and actual WAL-aware staging using five
   focused synthetic fixtures. No installed-profile acceptance claim.
2. Root wires strict renderer/plugin persistence generations and unsaved guard;
   this feature wires the new native registration/operation coordinator. Legacy
   and service owners add real drain/snapshot hooks in separately assigned files.
3. Implement explicit lossless converters/import digest on the existing candidate
   ReaderStore transaction. Preserve protected reader state and unresolved legacy
   records; no migration grant from raw paths/provider cache/review approval.
4. Root routes startup from the durable selector **before** opening SDKs, plugin
   stores, provider custody, diagnostics or daemon. Selector/activation receipts
   explicitly persist `legacy_current` versus `reader_only`; runtime mode is not
   inferred from a directory name or old receipt.
5. Add user review/activate/rollback orchestration and a real disposable packaged
   rehearsal, then separately audit the explicitly selected installed profile.
   Retain originals/archive and verify restart/uncertain-reply recovery. Only
   after those gates can generic CatDB runtime/dependencies be removed.
