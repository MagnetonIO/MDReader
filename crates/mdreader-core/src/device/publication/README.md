# Incoming document publication and retained revision transitions

This core owner prepares new immutable documents, replaces a selected current
revision, changes exact-revision pin/line state and retires documents while retaining
originals. It does not authenticate sync accounts, resolve remote head conflicts,
discover profiles, advance cursors, acknowledge receipts or activate sync.
Native/service integration and mobile/Windows acceptance remain separate gates.

## Owner contract

The native owner supplies a verified scope digest, remote item/revision UUIDs,
and fresh UUID activation/selection epochs in `IncomingOrigin`. Core compares
these exact fences; it does not authenticate caller claims. The application must
hold its selected-owner operation lease, revalidate current authority after waits,
and fence invalidation through the final transaction commit. Remote revision UUIDs
remain separate from the native raw-byte BLAKE3 revision and fresh local document UUID.

1. Build `IncomingDocument`/`IncomingAsset` outside the owner mutex. Exact UTF-8
   bytes, including BOM/CRLF/Unicode, are preserved. Names must be plain `.md`
   filenames; selected image targets must match Markdown AST image destinations.
2. `reserve_new` durably records the incoming origin and quota reservation before
   generated files. It reserves assets in the existing asset table, captures the
   generated document root and persists its identity. Exact retry preserves the
   local UUID; changed input, completed or dropped origins cannot regrant it.
3. Release owner access. `WritePlan::write` writes/fsyncs known generated names,
   accepts only an exact interrupted prefix, then reads/verifies retained files.
   `PreparedIncomingCopy::validate` fully decodes PNG/JPEG under one runtime
   validation permit outside the owner mutex. The permit survives until Ready drops.
4. Reacquire current selected-owner authority. `with_publication` verifies the
   live owner and origin, then supplies a scoped, non-escaping `PublicationGuard`
   and the exact borrowed owner connection. The application attaches its sync
   store to this connection; core opens no connection and owns no second writer.
5. Inside the caller's transaction, `publish_new` publishes the source grant,
   original document, validated assets, completed incoming intent and the existing
   published import-intent row. The latter preserves container-relocation recovery.
   Persist associations, receipt and cursor in the **same** transaction.
6. Run `revalidate_publication` after all callback/receipt/cursor writes,
   immediately before commit. Any error requires rollback of the entire transaction.
   Indexing follows commit; returned documents remain `indexed=false` until then.

A direct owner transaction has this shape (the service's real sync callback must
also perform its own authority and contiguous-cursor checks):

```rust,ignore
let receipt = with_publication(&mut owner, &ready, &current_origin, |connection, guard| {
    let transaction = connection.transaction()?;
    let receipt = publish_new(&transaction, &guard, &ready)?;
    // Persist exact association, receipt and cursor here using transaction.
    revalidate_publication(&transaction, &guard, &ready)?;
    transaction.commit()?;
    Ok::<_, PublicationError>(receipt)
})??;
// Schedule rebuilding/indexing after durable commit.
```

Final checks cover live-owner identity, exact borrowed connection, unchanged
origin/intent/document/import/source fields, enabled source, private-cache
separation, asset rows/initial manifest generation and retained private roots.
Files are reopened by their exact relative pathname and compared with retained
opened objects using device/inode/size/mtime/ctime evidence. Original bodies are
not rehashed or pixel-decoded while holding the owner transaction.

## Durability, bounds and recovery

`migrations/001-incoming-intents.sql` adds only the independently guarded
`device_incoming_schema_v1` and `device_incoming_intents_v1` staging namespace.
Existing applied document/import/asset migrations, library version 4 and index
version 5 remain unchanged. Document migration `003-device-state.sql` adds
monotonic state generations and explicit retirement on the same existing document
table, advancing its independent marker from 1 to 2 transactionally. Known marked
v1 and exact interrupted unmarked layouts preserve rows, files, pins, lines, intents
and grants. Existing rows start at generation zero; this records a migration
baseline, not reconstructed historical edits. Future or malformed layouts fail
closed without reset. Future document-schema fixtures must now use version 3.

The existing 1 MiB/document, 1,000-document/64 MiB library quota includes local
and incoming reservations, including dropping intents. Assets retain eight/
16 MiB per document, 4 MiB per image, 512 items/32 MiB library, decoder and runtime
bounds. A separate 4,096-intent history cap preserves completed/dropped origin
fences; `IntentLimit` requires deliberate future maintenance, not silent pruning.
The namespace/quota helper compiles without the optional image decoder.

Ordinary `recover_imports` never considers reserved incoming intents. Explicit
`resume` selects one persisted intent/origin and returns a `WritePlan`; `read`
performs bounded file reads outside the owner mutex. Files alone confer no grant.
Completed replay does not reactivate a source: reconcile the caller's committed
association/applied receipt, rather than invoking reservation/publication again.

Explicit discard is staged: `prepare_drop` durably fences publication and retires
reserved assets; `DropPlan::remove_files` removes/fsyncs only known generated
copies; `finish_drop` releases quota and retains a dropped-origin fence. The caller
must quiesce issued writers under its operation lease before cleanup. Unknown
files and empty ungranted document leaves are retained; there is no recursive
cleanup. Failed cleanup retains its reservation. No automatic cleanup is enabled.
Older clients cannot count this new namespace: resolve/export active intents
before downgrading an incoming profile.

Unix publication is implemented. Windows retained-file evidence now uses full NTFS
identity/change stamps and protected current-user DACLs; source paths use the same
Windows-wide encoding as the durable store. Windows reservation/resume/cleanup
still refuses before file mutation with `UnsupportedPlatform`: durable namespace
creation/write/removal adapters and Windows runtime acceptance remain required.

## Immutable transitions and archival access

`prepare_current(owner, documentId, assets, origin)` captures bounded current
metadata and retained roots under short owner access. `CurrentPlan::verify` reads
and hashes exact Markdown bytes outside owner access. The resulting opaque
`VerifiedCurrent` binds the native revision, pin/line generation, asset rows and
manifest generation to the live owner and exact incoming activation/selection.
`with_transition` supplies a non-escaping guard for the actual borrowed connection.

Inside one caller transaction:

- `revalidate_current` is the read-only final fence for an external selected-document
  association/outbox record. It requires the already-adopted exact v2 namespace;
  old, future, missing or partial namespaces are rejected without migration or
  repair. It delegates complete owner/connection/origin, retained file/root,
  source permission/cache separation, document generation/state and asset
  metadata/generation checks to the existing current-state validator. It does
  not publish, regrant, update pins/positions, commit, index or authenticate a
  remote session. Call it before admission and again after **all** caller SQL,
  immediately before commit; any error requires whole-transaction rollback.
  The service must separately retain its actual current authority/selection
  lease through that commit and verify selected asset bytes outside the lock.
- `publish_state` applies a `StateUpdate` under generation CAS. Positions are
  `Preserve`, `Start` or a bounded exact line. Local `device::pin` and `position`
  use the same monotonic generation rule, so changing a pin and changing it back
  cannot erase intervening edits. No-op writes also advance generation. Device source disable/re-enable advances
  the same generation atomically with permission changes. Exhaustion returns
  typed `StateLimit` for document transitions without wrap/reset; disabling
  remains durable at the saturated maximum, while re-enable is refused.
- `retire_current` records an explicit `RetirementRecord`, disables the old source,
  invalidates its index job and retires assets without deleting their copies. The
  remote operation UUID must match the selected origin.
- `with_replacement`/`publish_replacement` publish a prepared fresh local document
  and source, then retire the selected old document atomically. Pin state copies
  exactly; the new position starts at line 1, including identical-body revisions.
  Neither the remote wire UUID nor raw BLAKE3 equality is a native identity alias.

After **all** outer receipt/head/cursor SQL, call the corresponding final function
`revalidate_state`, `revalidate_retirement` or `revalidate_replacement` immediately
before commit. Errors require whole-transaction rollback. Final checks cover
owner/connection/origin, exact generation/state, enabled or retired source status,
source/root metadata, cache separation, unchanged file evidence and asset state.
No body hashing, pixel decode, independent commit or indexing occurs there. The
caller still fences current remote authority through commit and resolves conflicts.

The narrow shared-file integrations are `ReaderStore::stored_root` calling the
private device authority predicate and `set_source_enabled` delegating only
device-owned sources to its transactional generation/permission helper. It rejects retired or unsupported device-owned
sources for generic read/search/reindex/re-enable; unrelated local/provider grants
retain existing semantics. Device list/recovery also omit retired owners. Old
exact evidence becomes unavailable without deleting its index or original files.

`prepare_archive(owner, explicitDocumentId, assets, runtime)` selects retained
metadata without granting a source, changing state, discovering files or activating
sync. `ArchivePlan::read` reads and hashes known files outside the owner lock. Its
opaque result contains exact UTF-8 Markdown bytes, document state/retirement and
tracked asset metadata with available bytes. Missing assets remain `None`; partial
copies preserve exact available prefixes. Complete known images are SHA256 checked.
The shared runtime limits concurrent archive/view snapshots to two. This is a
local export read contract, not an archive file writer or a current citation grant.

Retained document rows and all ready/reserved/retired asset rows continue counting
existing quotas. There is no automatic pruning or content loss. Explicit later
asset Forget remains a separate existing action. A downgrade after schema2
requires export/backup and an explicit migration decision: this slice supplies no
reverse schema migration, source regrant, filesystem overwrite or graph replay.
Ordinary container-relocation recovery skips disabled/retired sources. If an iOS
update moves the container, an archived source's stored old path may no longer
resolve even though its bytes remain retained. Explicit archive-only relocation
using the persisted parent/root identities is a separate required lifecycle gate;
this slice never fixes an archive path by silently regranting or activating it.
Service replacement/state/deletion orchestration and actual product acceptance
remain separate gates; injectable/core fixtures cannot establish authenticated
end-to-end sync.

## Focused evidence from the preceding new-document checkpoint

Five real temporary-SQLite tests passed: exact originals/assets plus outer rollback
and reopen; interrupted copies/local recovery isolation and explicit resume; epoch,
wrong-connection and local-ID collision fences; post-callback source revocation,
private-cache overlap and asset-generation changes; file/root changes and pixel
decode refusal; shared quota, staged discard and future namespace preservation.
Several assertions share cohesive lifecycle tests; this is five tests, not a
claim of separate release scenarios. Tests followed code-only sequencing; no
preimplementation behavioral RED was observed.

With the coordinator's sole build slot and `CARGO_BUILD_JOBS=2`:

```sh
cargo test --locked -p mdreader-core --features device-assets --lib device::publication::tests
cargo clippy --locked -p mdreader-core --all-targets --features device-assets -- -D warnings
```

Observed: five tests passed (0.18 s); strict Clippy passed (2.44 s). Default-core
integration, existing asset behavior tests after decoder factoring, native
projection/cursor composition, packaging and live release acceptance are not
claimed by this focused checkpoint. No dependencies, lock changes, private
profiles or live sync operations were involved.


## Transition checkpoint evidence

Five additional real SQLite/filesystem tests passed: marked-v1 transactional
upgrade/reopen preserving populated disabled rows; pin ABA plus wrong-owner and
post-callback fences; exact-line bounds and generation exhaustion; replacement
rollback/commit/reopen with identical raw bytes, preserved pin/reset position,
quota accounting and retained export; retirement revocation/asset-session release
and unrelated local source enable/read preservation.

```sh
cargo test --locked -p mdreader-core --features device-assets --lib device::publication::tests::transitions
cargo test --locked -p mdreader-core --features device-assets --lib device::schema_tests::marked_v1_state_upgrade
```

With the coordinator's sole slot, `CARGO_BUILD_JOBS=2` and the existing warm
benchmark target, a deliberate omission of the device-only permission dispatch
failed the grant-ABA case: generation stayed 2 instead of advancing to 4 (0.04 s).
The dispatch was immediately restored. Four transition tests passed (0.14 s),
then the marked-v1 migration test passed (0.03 s). The affected device suite passed
all 28 tests (0.82 s), and feature all-target strict Clippy passed (2.32 s):

```sh
cargo test --locked -p mdreader-core --features device-assets --lib device::
cargo clippy --locked -p mdreader-core --all-targets --features device-assets -- -D warnings
```

No new dependencies, manifests, lock changes or private-profile access accompany
this transition slice. Default-core combined-tree checks, native schema fixture
update, service replacement/state/deletion orchestration, archive relocation and
product/live acceptance remain separate gates. No full-workspace or model tests
were run for this checkpoint.

## Read-only selected-association fence checkpoint (2026-10-08)

Focused synthetic real-SQLite evidence, warm target, `CARGO_BUILD_JOBS=2`:

```sh
cargo test --locked --offline -p mdreader-core --features device-assets --lib device::publication::tests::current_fence:: -- --nocapture
cargo test --locked --offline -p mdreader-core --features device-assets --lib device::publication::tests:: -- --nocapture
cargo clippy --locked --offline -p mdreader-core --features device-assets --all-targets -- -D warnings
```

The new two cases passed in 0.08s (compile 3.45s). They prove that provisional
caller associations roll back with changed source grants, asset state or document
generation; successful association/reopen preserves the existing document, exact
BOM/CRLF bytes and user state. The final fence itself changes neither SQLite's
write count nor schema version. A coherent old v1 layout, future marker, missing
marker and partial namespace are all rejected without repair.

Temporarily removing only the v2 precondition produced the expected RED (0.06s):
the compatibility initializer silently upgraded the coherent v1 namespace and
accepted the fence. The exact source was restored byte-for-byte; the affected
publication module then passed all 11 cases in 0.32s (compile 0.97s). Strict
all-target core/device-assets Clippy passed in 2.57s. No schema migration,
dependency, lockfile or protocol changed. This seam does not itself implement
outgoing native-to-wire association, self-echo, QueueDocument RPC or UI.

## Retained archive file evidence checkpoint (2026-10-08)

`ArchivedDocument` now carries its original owner, exact asset metadata and opened
Markdown/asset file evidence until the selected snapshot is dropped. The new
`revalidate_current_archive(tx, guard, current, archive)` fence verifies those
identities on the actual caller-owned transaction, without hash/decode work,
writes, migration, new admission or commit. Invoke before provisional association
and immediately before final commit. Ordinary incomplete archival exports remain
possible; current publication refuses missing, partial or nonready resources.

Two real-SQLite regressions passed (4.67s compile, 0.15s tests), covering in-place
asset edits, equal-byte file replacement, deletion/root replacement, same rows
under another actual owner, changed asset generation, incomplete archive export
and read-only successful final checks. Provisional caller associations roll back
on failure. Temporarily omitting only the final retained evidence loop produced
expected RED: the changed asset was accepted (1.84s compile, 0.08s test). Exact
production bytes were restored; all thirteen affected publication cases passed
(1.74s compile, 0.49s tests), then strict core `device-assets --all-targets` Clippy
passed in 3.30s. Two jobs, locked/offline, existing warm benchmark target and one
heavyweight slot were used; no model/full-workspace suite.

This closes the core evidence-lifetime gap. Native service association/self-echo
callers must adopt both fences before product upload exposure; that acceptance,
OS-vault/live sync and release packaging are not established by these tests.
