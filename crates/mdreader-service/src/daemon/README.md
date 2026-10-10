# ReaderDaemon live-owner drain

This owner is issued only by the actual headless startup path: `reader.lock` is
acquired before opening/migrating SQLite, then transferred into Runtime's
admission owner. Standalone/mobile Runtime constructors and unsupported hosts
retain ordinary operation but cannot issue `FrozenReaderDaemon`. There is no
new RPC, selector activation, installed-profile mutation or all-eight readiness.

Every dispatched request except storage-free Ping and every scheduler turn owns
an admitted effect. Native model and asynchronous provider commands retain
forked effects until actual work, buffered completion and timeout/Unknown cleanup
finish. An abandoned reply or model RestartRequired state does not settle an
issued effect. Already-admitted descendants may fork while new admission is
denied; this does not admit an unrelated request. The startup lease remains
retained by command effects even after Runtime is dropped.

`Coordinator::freeze(operation, owner_epoch, deadline)` accepts UUIDv4 operation
and owner epochs and a positive deadline of at most 30 seconds. Its Condvar
wait holds no ReaderStore mutex. After effects settle it checks both actual
writer and readonly query connections, including each index connection's actual
`reader_authority` SQLite handle. VFS `HAS_MOVED`, retained nofollow single-link
stat identity and weak live-owner tokens cover library/index pathname and
opened-handle binding without opening/closing another database descriptor.
The original process lock, PID and root are revalidated too. Unix and Windows
have native preservation paths. Windows checks the borrowed SQLite HANDLE and
full NTFS identities, retains the actual reader byte-lock, and drains the same
writer/query admission gate. Other hosts cannot issue this capability. Native
Windows fixture and installed-process acceptance must be recorded separately.

While denied, the scheduler waits on admission resume/shutdown notification,
with no timed polling. Ordinary idle scheduling retains its existing backoff.
A successful frozen/resume cycle rotates only the volatile scheduler epoch:
pending journals/index jobs remain durable, but the next admitted turn discards
buffered old-epoch vectors before installation. Successful proof requires zero
active effects, so no old scheduler turn can overlap that proof/resumption.
A failed drain yields no preservation/cutover authority. Its denial drop rotates
the epoch without subtracting still-running effects; an already-admitted turn
may finish under existing exact root/revision/generation checks, then its next
turn discards volatile old tickets. A later freeze must still wait for actual work. Root/handle/lock uncertainty,
poison or invalid frozen teardown quarantines admission instead of reporting
resumption. Runtime shutdown wakes the worker and cannot certify a dead runtime.

A frozen capability permits an exact scoped borrow of the existing readonly
library connection for a trusted snapshot adapter, after revalidation. It does
not open another SQLite file, export a writer, checkpoint WAL or promise a
consistent raw-file copy. Account actor work, device/custody tasks, external
capture hooks/spool locks, integrations and legacy writers need their independent
actual capabilities. ReaderDaemon alone is not whole-profile quiescence.

The four synthetic/public fixtures exercise actual Runtime/SQLite/process lease
ownership, transferred descendants, a blocked native factory past timeout,
old-epoch buffered inference, index/process-lock replacement, and accepted
provider cleanup after a lost reply. Only the model/provider boundary is
synthetic; no model download, private profile or network is involved. Existing
core identity coverage also checks the actual attached authority handle.

## Focused checkpoint

Run from `feat/reader-profile-transition`, base
`3d9a5f0fcba2d3af14d69bbe68632ec5c3f55fb2`, with
`CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/Users/mlong/Library/Caches/MDReader/benchmark-target`:

- `cargo test --locked --offline -p mdreader-service --lib daemon_tests -- --nocapture`:
  four passed, final restored source, 0.22s test execution.
- `cargo test --locked --offline -p mdreader-core --lib store::identity::tests -- --nocapture`:
  three passed, 0.01s; includes actual writer/read-only startup and attachment
  substitution plus the existing subprocess SQLite lock regression.
- `cargo clippy --locked --offline -p mdreader-service --all-targets -- -D warnings`:
  passed, 3.76s.
- Same strict Clippy command with `--features sync-native`: passed, 5.44s.

The meaningful RED deliberately dropped a model command's effect before native
factory execution. The blocked-factory fixture then failed its exact expected
freeze-Deadline assertion: a ticket timeout incorrectly appeared drained. The
model source was restored byte-for-byte (SHA256
`52ab8ad6579ea70ddc0aad0e69c762eaf60ca072de9782fed22b1b45ee6d7e70`)
before the four-fixture GREEN rerun. The first compiler pass also exposed an
omitted daemon module declaration, corrected before these behavior checks; that
compiler failure is not behavioral RED evidence.

No dependency/lock/schema changes, full suite, native packaging, model download,
installed-profile access or all-owner preservation acceptance ran here. Actual
startup-lock replacement is covered in-process, not an exhaustive cross-process
startup-lock acceptance matrix. The single heavy slot was released after both
strict configurations completed.
