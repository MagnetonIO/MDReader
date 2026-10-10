# Capture producer preservation seam

Source checkpoint based on merge `a18769d` (integration `3f197ca` plus the
reviewed profile bundle work). This is a real service admission/OS-lock seam,
not profile-wide migration readiness. No installed histories are read and no
activation, selector, session, or source grant is changed by freeze.

`Runtime::capture_participant()` returns the exact `Arc<Coordinator>` also used
by its capture worker and request dispatcher. Future preservation composition
must use that handle in the actual service process. Constructing a second gate
beside a running Runtime cannot freeze its effects. The constructor binds the
actual writer Arc and core's opaque `LibraryIdentity`: the selected root and
original library identity captured from the name with SQLite's actual Unix
VFS-open object checks bracketing capture, before owner startup succeeds.
`ReaderStore::library_identity()` repeats that check on the actual borrowed
connection at capture construction. It does not open another SQLite connection.
Reopening/replacing the owner requires a new native coordinator lifecycle.

The previously documented open→constructor replacement gap is rejected rather
than adopted. Path/stat-only checks also cannot exclude opening B between an
A→B→A pathname substitution; the tiny audited core FFI adapter requires
`SQLITE_FCNTL_HAS_MOVED` to return `SQLITE_OK` and exactly zero. Missing/unsupported
VFS capability fails closed. Cloned identity capabilities retain a weak
live-owner proof and original root/stat identity; SQLite itself holds the original DB descriptor. No extra
DB descriptor is opened or closed for certification (POSIX close can silently
release process-wide SQLite locks). Owner drop expires every prior certificate;
reopen produces a fresh one. Subsequent verification uses descriptor-relative
nofollow stat, without opening a DB descriptor; capture effects and freeze
compare the current name to that original. The weak liveness token is never
upgraded and is declared before both SQLite connections in ReaderStore, so it
expires before connection teardown. Existing symlinks/multiple-link files are
rejected by preflight before opening SQLite; fresh absent libraries are still
created only by SQLite.
The filesystem capability has Unix and Windows implementations. Windows binds
the actual SQLite HANDLE, full 128-bit NTFS identities and weak live-owner
token, verifies protected spool directories, and acquires the real queue byte
lock. Other hosts retain the filename association check and cannot freeze.
Windows service freeze/backup fixtures are authored; runtime acceptance remains
separate from source and compilation.
SQLite's existing create/configuration/migration/permissions behavior is unchanged;
no separate file creator or schema was introduced.
See [SQLite's file-control contract](https://www.sqlite.org/c3ref/c_fcntl_begin_atomic_write.html#sqlitefcntlhasmoved).

This closes the actual-opened-file startup binding gap; it does **not** freeze
ReaderDaemon work or certify all profile writers. Its owner must still be the
same actual writer Arc, and the borrowed connection API must not be used to
replace the owner's connection. ReaderDaemon drain, all-eight participant
composition and server-retained cross-process preservation remain separate gates.
No selector, activation or all-eight readiness is enabled by this change.

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


## Opened-library identity checkpoint (2026-10-08)

This follow-on closes the named-file adoption gap recorded in `302b0e1` without
changing service Runtime wiring, SQLite migrations, dependencies or lockfiles.
The exported capability is not a ReaderDaemon drain or all-eight backup proof.
Other hosts retain ordinary constructor behavior; no Windows certification or
cross-host runtime acceptance is claimed here.

Using the same warm target and two jobs, locked/offline:

```sh
cargo test --locked --offline -p mdreader-core --lib store::identity::tests
cargo test --locked --offline -p mdreader-service --lib capture::admission::tests
cargo clippy --locked --offline -p mdreader-core -p mdreader-service --all-targets -- -D warnings
cargo clippy --locked --offline -p mdreader-service --features sync-native --all-targets -- -D warnings
```

Final core identity fixtures: 3 passed (0.01s, 0.88s compile), covering ordinary
writer/readonly/reopen, old-certificate expiry, wrong selected root, alias
preflight, post-open replacement and actual-open ABA. Capture admission fixtures:
3 passed (0.17s, 4.42s compile), including replacement before construction
(rejected before creating the spool) and afterward (quarantine). Strict default
core/service Clippy passed (6.51s); shipping service `sync-native` Clippy passed
(6.35s). No broad/model/native/package or installed-profile checks ran.

Meaningful ABA RED: removing only the `moved != 0` refusal caused the ABA test
to accept SQLite-open B while the original path A had been restored. The exact
pre-mutation bytes were restored (SHA256
`3cef83e0f7ec0e7f0d0ba0301a15aa814f84b87a71ff03e20fc6eb7d3b105731`);
subsequent liveness/preflight refinements change that hash. Final ABA stays GREEN.

Meaningful POSIX-lock RED: injecting an extra main-DB `File::open`/drop into the
identity stat path let an independent child process acquire `BEGIN IMMEDIATE`
while the parent transaction was still held. Exact restoration (SHA256
`a5d769b2ad6103455d9002fb61029709284a3c5b9f705c75c97566464c7ca319`)
then returned GREEN. The fixture uses the actual core certification helper and
real SQLite rollback journaling, because WAL's shm lock would mask this bug.
An initial attempt to switch a fully composed ReaderStore from WAL to DELETE
was refused Busy by its attached authority connection; the fixture setup was
corrected to invoke the same helper directly, with no production relaxation.

Artifacts: `benchmark-target/debug/deps/mdreader_core-be42c800f520936b` and
`benchmark-target/debug/deps/mdreader_service-7217306da51abc38` in the previously
selected external MDReader cache. Source remains scoped to core's new identity
module/store composition and capture admission/capability tests/docs.
