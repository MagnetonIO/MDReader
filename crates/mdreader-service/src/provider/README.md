# Service provider registry and orchestration (stage 3)

This module borrows the existing library owner's SQLite connection. It never
opens a database file, registers a remote cache as a local source, performs OAuth,
loads private histories, or changes client/provider credentials. The direct and
process tests use synthetic accounts/transports and temporary files only.

## Owner and core boundary

`Owner::with_connection` serializes short synchronous operations. Implementations
are provided for the existing `Mutex<ReaderStore>` and an already-owned
`Mutex<Connection>`. `synchronize`, `fetch_and_admit`, `dispatch_write` and
`repair_mutations` await provider I/O between closures, never inside a writer
closure. `RegistryAuthority<Owner>` implements the provider kernel's admission
callback. Inject the service-owned authority when constructing a native client.
The orchestration wrapper also imposes operation cancellation/deadlines on an
injected provider that fails to enforce its context.

`ProjectionAdmission::{admit,remove,revoke}` receives the same borrowed
`rusqlite::Transaction` as registry/cache publication. `admit` must publish durable
Markdown authority and an exact binding/revision-bound index job before returning
`ProjectionReceipt {job_id,reader_source_id,path,revision}`. `revoke` is a pure SQL
access gate inside the **same transaction** as registry disable: cached read,
search, MCP and pending index jobs must stop being authoritative atomically.
Physical cleanup is separate from revocation. Any failed authoritative callback
rolls the transaction back and reports failure; it never reports a false success.

`UnsupportedProjection` refuses admission/removal/revocation. A source that has
never admitted a core projection can be disabled without a core callback. Once
`projection_bound` is durable, the authoritative revoke callback is mandatory.
Unsupported admission may leave an explicitly private, observed cached body;
it never creates searchable coverage or a local-folder grant.

`CoreProjection::open(absolute_private_cache_root, library_id)` is the concrete
bridge to `mdreader_core::ProviderProjection`. The service gets `library_id` from
its own durable registry metadata, not from a renderer. Core derives a separate
reader source ID `provider:<full binding digest>` for each generation, a generated
safe `records/<key digest>.md` path, and an exact revision incorporating remote
version plus actual body hash. The original remote key is never a pathname.
Core's authority gates and durable jobs are committed together with registry
publication. The bridge maps errors to closed, redacted provider errors.
Non-Unix hosts fail `Unsupported` before creating cache paths pending private ACL
and durable-cache runtime acceptance. SQLite-only registry primitives are portable.

## Registry and lifecycle

`library_id(connection)` generates/persists one private profile identity using the
module's own schema guard. No global `user_version`, journal mode or synchronous
policy is changed; FULL/EXTRA synchronous durability is required. Unknown/partial
registry schemas fail closed. All tables are `mdreader_provider_registry_*`,
separate from provider mutation/core/mobile namespaces.

`admit(connection, descriptor, credential_reference, configuration_digest)` is
explicit initial admission. It rejects foreign-library descriptors and existing
source IDs; it never implicitly regrants. Names/selected scopes/capabilities are
validated bounded provider DTOs. Persisted descriptor/coverage is normalized to
Ready/NeverIndexed, so caller-supplied completed coverage is not trusted. Store
only an opaque private credential **reference**, never a token or raw credential.
Actual grant custody and narrow OAuth scope verification remain platform-owned.

`configuration(connection, source_id)` is private non-serializable factory input:
descriptor, credential reference, configuration digest. `list(connection, library)`
returns bounded sanitized source summaries: registry source ID, optional derived
reader source ID, name/kind, enabled/health, selected scope kind/count/recursion,
capabilities and honest coverage. It does not return account identity, selected
provider IDs, credential references, cursors, bodies or private configuration.

`disable` atomically revokes core authority and pauses the registry. In-flight
operations fail their binding checks before publication. `reconnect` is explicit,
requires prior disabled authority, and requires a different generation. Account,
selection, auth profile/resource-key authority or configuration changes require
this fresh generation. Old service caches/snapshots are retired, while unresolved
mutation bodies/receipts/conflicts stay under their old binding and cannot be
consumed by the new account. `forget_materialization` disables authority and drops
derived service cache/metadata/cursors, retaining paused selection and durable
mutation evidence. No lifecycle method logs out, deletes OAuth credentials or
automatically expands permission scope.

## Snapshots, cursors and bodies

`begin_snapshot` takes a change checkpoint obtained **before** initial metadata
enumeration. `stage_page` binds each page to the expected committed continuation;
consumed/cyclic/nonadvancing cursors are rejected. Staging and published snapshots
are separate, and `resume` survives process death without exposing incomplete
rows. For changes, resolve each upsert via provider metadata before `stage_changes`.
Deletion with `expected_path` affects the stable ID only when its staged locator
still matches; old-path tombstones cannot delete a renamed current document.

`publish_snapshot` atomically replaces published metadata and checkpoint. Core
removal fences previously admitted records whose exact version disappeared or
changed before the new checkpoint commits. Any callback failure keeps old metadata,
cursor and core authority together. A source publication epoch prevents a snapshot
staged before an accepted write from overwriting that write's newer local head.
A subsequent `synchronize` discards stale staging and starts from the published
checkpoint. `reset_for_rebuild` drops invalid staging, clears the rejected cursor,
and marks old observations Partial; the next synchronization obtains a new
checkpoint and full enumeration rather than repeatedly polling an expired cursor.

Final metadata enumeration gives `MetadataCompleteSelected`, never completed
body/index coverage. Partial enumeration remains partial even after a final change
checkpoint. Provider search is not completeness evidence. This stage deliberately
does not claim `CompleteSelected`; index-coverage aggregation remains an owner gate.

`cache_body` validates exact currently published key/version, declared length,
UTF-8 and tagged provider hash, then persists bounded bytes/hash/observation time.
An immutable revision cannot silently acquire different bytes. `cached` returns
only that current published version and explicit observation time; it is not proof
of present remote head. Cache-only refresh preserves an existing admitted receipt.
`admit_body` additionally commits a core projection receipt/job in the transaction.
`fetch_and_admit` combines provider read, durable cache, and explicit core admission.

Limits: 128 admitted source records, 16,384 rows per staged snapshot, 512 pages per
snapshot/cursor kind, 32 KiB metadata/receipt records, 64 MiB total metadata,
256 MiB service body cache, 2 MiB UTF-8 bodies, provider cursor bounds of 64 KiB.
No unbounded startup source scan or automatic model download occurs here.

## Guarded writes and repair

`prepare_write` compares the exact published version and durably commits intended
body through `mdreader_provider::journal`. `dispatch_write` explicitly obtains the
single-use permit after another binding/version check, performs one provider
write outside the writer, and validates receipt operation/key/body hash/length
and tagged provider hash. A verified receipt remains durable if source authority
was revoked during I/O; revocation prevents local publication. An ambiguous
response or interrupted dispatched intent is Unknown and never automatically sent
again, including after account switch or restart.

`publish_mutation` atomically admits exact new metadata/cache/core job, then marks
the accepted journal entry Published. A process stop between those commits is
repaired idempotently from the stored receipt. Third-party published heads are not
overwritten by an old accepted receipt. `repair_mutations` converts interrupted
Dispatched to Unknown, performs only reads for uncertain outcomes, retains both
bodies for conflicts, and retries local publication for Accepted. Matching bytes
establish observed convergence, not causal proof of authorship. Prepared intents
require a separate explicit dispatch request; startup repair never uploads.

## Verification and outstanding integration

```sh
CARGO_TARGET_DIR=/Users/mlong/Library/Caches/MDReader/provider-target cargo test -p mdreader-service --test provider
CARGO_TARGET_DIR=/Users/mlong/Library/Caches/MDReader/provider-target cargo clippy -p mdreader-service --test provider --no-deps -- -D warnings
```

Focused regressions cover owner schema/library admission, honest partial coverage,
reopen/resumable publication, cursor reset, path-fenced tombstones, atomic callback
failure, deadlines/cancellation with no held writer, private-cache versus admission,
account switching/revocation, send-once uncertainty/conflicts/forget, a subprocess
killed after committed staging+dispatch with two reopen repair phases, and the
actual core bridge's index/exact read/readonly search/atomic revoke.

Initial admission, partial-coverage promotion, and conflict-repair behavior were
observed RED before their fixes. This stage does not wire runtime scheduling or
IPC/app requests, replace legacy Library/sidebar/settings queries, migrate legacy
profiles, construct credential resolvers/cursor-key custody, prove OAuth/Picker UX,
perform live provider access/writes, implement assets, or establish complete
searchable coverage. Those gates remain explicit; this is not Task 6 acceptance.
