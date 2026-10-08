# Capture producer preservation seam

Source checkpoint based on merge `a18769d` (integration `3f197ca` plus the
reviewed profile bundle work). This is a real service admission/OS-lock seam,
not profile-wide migration readiness. No installed histories are read and no
activation, selector, session, or source grant is changed by freeze.

`Runtime::capture_participant()` returns the exact `Arc<Coordinator>` also used
by its capture worker and request dispatcher. Future preservation composition
must use that handle in the actual service process. Constructing a second gate
beside a running Runtime cannot freeze its effects. The constructor binds the
actual writer Arc, the connection's selected `library.sqlite` pathname, a
retained library file and the retained selected profile root. It does not open
another SQLite connection. Reopening/replacing the owner requires a new native
coordinator lifecycle.

Startup binding limitation: core ReaderStore currently exposes its SQLite
connection filename, not a retained identity of the object opened by SQLite.
The constructor compares that filename and retains the **currently named**
library inode. It cannot prove the SQLite handle still names that inode if
replacement raced between `ReaderStore::open` and construction. The separate
ReaderDaemon composition must close this gap before all-eight preservation can
be enabled; this capture capability is not a full writer/root backup proof.

On Unix construction initializes the empty private spool if necessary, then
retains its root, four child directories and queue-file identity for the selected
service lifetime. This creates no source grant or project opt-in. These identities
are checked before ordinary effects and freeze, so a pre-freeze directory
replacement cannot be silently adopted and omit original queued bytes. A Busy
open is not proof: constructor identity capture is allowed while that real
writer is running, and freeze still must acquire its exclusive lock afterward.

Each actual Runtime path takes a noncloneable `EffectLease` before taking the
reader mutex and retains it through its synchronous accepted/error/uncertain
result:

- Background `capture::drain`: project, durable capture/job commit, acknowledge.
- Configure capture: policy artifact publication followed by durable DB policy.
- All explicit backfill requests: review/state can initialize policy directories;
  enqueue/deselect publish selection and possibly the durable queue event.
- Integration prepare/apply/uninstall: preparation SQL, private installer state,
  receipts and explicit external client configuration mutations.
- Capture status: `Spool::open` may initialize missing spool directories.

The capture policy and integration listing queries do not mutate files and
remain read-only. Reader indexing, catalog, provider, account, review and other
owner work is **not** denied by this gate. Their separate real participant
capabilities must also be held before any profile-wide snapshot is published.

`Arc<Coordinator>::freeze(operation, owner_epoch, deadline)` is blocking and
native-only. UUIDs must be fresh native v4 operation/owner identifiers and the
remaining deadline at most 30 seconds. It denies new service effects, waits for
actual issued leases, then acquires the existing exclusive `queue.lock` with
bounded Busy-only retries. It holds no ReaderStore mutex while waiting. Spool
initialization finishes before returning any capability; freeze does not require
the queue to be empty, run models/indexing, remove temporary files, or acknowledge
uncertain captures. Pending events, Markdown, receipts and interrupted temporary
writes remain byte-for-byte available for preservation and later replay.

`Spool::freeze()` consumes the actual spool and retains the same lock used by
hook subprocess `open/enqueue`, projection, acknowledge and temporary recovery.
It also retains the root, four namespace directories and lock object. Final
revalidation reopens exact pathnames and compares their identities with those
objects, rejecting symlink/unlink/rename replacement. Unix is the initial
supported preservation host; other hosts explicitly refuse freeze. Existing
non-Unix spool durability gates are not weakened.

`FrozenCapture::revalidate(coordinator, owner, operation, owner_epoch)` verifies
that exact live service/owner, native tag, deadline, namespace and drained state.
It provides no database writer or serialized readiness proof. Valid
preservation-only Drop releases the OS exclusion before reopening service
admission. A deadline during partial drain returns no proof and resumes valid
old admission without canceling the still-issued effect. Changed authority,
poisoned state or uncertain lock release quarantines the service gate and retains
the acquired OS-lock guard. An unlock error is uncertain; retaining its handle
does not assert either resumed or held kernel exclusion. There is no automatic
clear/rebind API.

## Producer audit and remaining composition

The shipped packaged and standalone hook/backfill entrypoints call
`mdreader_capture::cli::run`, then the actual queue-locked Spool API. They never
publish policy artifacts, integration receipts, or SQLite state. The packaged
`--mdreader-capture` path in `src-tauri/src/main.rs` uses that same implementation.
The standalone integrations crate currently supplies a library, not an
independently shipped installer CLI; the shipped native prepare/apply/uninstall
requests route through Runtime. Direct embeddings of public `capture` or
`integrations` functions still must carry their registered actual effect lease;
this seam cannot certify arbitrary third-party callers or noncooperating
filesystem editors. Existing core capture policy APIs are owner operations, not
an independent producer-discovery mechanism.

Cross-process native preservation must retain this real guard **server-side**
under its authenticated native operation/root/session lease and perform the
scoped backup/export there. A TCP acknowledgement or renderer boolean cannot
transfer a Rust guard, prove a service has stopped, or attest all other writers.
That service preservation protocol, all-eight participant assembly, installed
profile acceptance, cross-host acceptance and durable selector/restart handoff
remain pending. No production preservation RPC is registered by this slice.

## Focused evidence (2026-10-08)

All commands used the existing `benchmark-target`, `CARGO_BUILD_JOBS=2`,
`--locked --offline`; no native/model/package/install or private profile run.

```sh
cargo test --locked --offline -p mdreader-service --lib capture::admission::tests
cargo test --locked --offline -p mdreader-service --lib runtime::capture_admission_tests::actual_runtime_capture_and_integration_paths_are_denied_while_frozen -- --exact
cargo test --locked --offline -p mdreader-capture --test hook_process preservation_lock_fences_real_hooks_and_keeps_interrupted_receipt_bytes -- --exact
cargo clippy --locked --offline -p mdreader-capture -p mdreader-service --all-targets -- -D warnings
cargo clippy --locked --offline -p mdreader-service --features sync-native --all-targets -- -D warnings
```

Admission fixtures: 2 passed (0.08s, initial compilation 14.08s), covering actual
issued-effect/deadline/owner-mutex drain and before/during-freeze namespace
replacement with retained OS-lock quarantine. Runtime fixture: 1 passed (0.07s,
1.17s compilation) covering actual configure/backfill/integration admission.
Hook subprocess fixture: 1 passed (1.09s, 1.23s compilation), including bounded
hook refusal during freeze, exact pending/receipt/temporary byte preservation
and replay after release. No grants are created by startup identity binding:
constructor SQL is read-only; source admission remains explicit ConfigureCapture.

Meaningful mutation RED: removed only the actual ConfigureCapture lease; the
Runtime fixture failed because disabled policy publication was accepted while
frozen. Restored exact pre-mutation Runtime bytes (SHA256
`e6c5ccb96047b3adc20c175c5e95d312f2739473fd3b9b661c8383c031919580`), then
the same fixture passed. Later constructor formatting/lint correction changes
that file hash; the restoration receipt describes the mutation boundary only.

Affected strict Clippy passed for default capture/service (3.74s) and shipping
service `sync-native` (4.86s). Initial default Clippy exposed two pre-existing
`needless_update` warnings in NativeAdapters factory-only constructors. The
approved narrow fix explicitly initializes cfg-native accounts to None while
preserving factory/configured account paths; no lint suppression or dependency
change was introduced. Existing core, account, native and integration suites
were not rerun for this bounded seam.

Artifacts in `/Users/mlong/Library/Caches/MDReader/benchmark-target/debug`:
`deps/mdreader_service-7217306da51abc38`,
`deps/hook_process-64b15e6a87b99f32` and `mdreader-capture`.
