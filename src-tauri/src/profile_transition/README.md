# Native profile writer admission and strict JSON ownership

The original portable foundation was based on integration `e766942`. The latest
Unix desktop startup composes the real session/settings JSON owner and removes
the plugin's exit-time writer for those hosts. Six other actual participants
remain missing; SDK, daemon, editor and hook quiescence are not established.
Installed preservation and selector activation/rollback remain unavailable.
Windows/mobile retain ordinary plugin-backed persistence and do not acquire
installed-preservation authority. See the current
[native startup receipt](../../../docs/development/native-strict-persistence-2026-10-08.md).
No installed profile was read or changed during validation.

`NativeProfileTransition::new(&Inventory)` binds one explicitly captured native
profile root and mints a fresh process owner epoch. Composition registers exactly
the eight `REQUIRED_PARTICIPANTS` through real `LiveParticipant` adapters. Each
registration declares its bounded, exact relative SQLite paths; duplicates,
another root or replacing a registration are rejected. Registration seals when
the first ordinary effect or preservation starts. There is no user/renderer
readiness boolean, public freeze token or inferred participant discovery.

Every actual writer calls `effect(participant)` before issuing work and moves
the noncloneable `EffectLease` into its task. It drops the lease only after the
effect finishes, including a durable accepted/unknown remote receipt. A task
cancellation request, lost IPC reply or closed window is not completion. The
shared admission bounds active leases at 1024. Existing producers that bypass it
remain an explicit missing integration gate, regardless of this API's behavior.

`preserve(choices, destination, limits, timeout)` takes one exclusive transition
try-lock, denies new effects and drains already issued leases using a bounded
condition-variable wait (maximum 30 seconds). It holds no DB/participant mutex
while waiting. Then every real participant must deny its independent producers,
join issued work and strictly flush state under the supplied `FreezeScope`.
Scope getters expose exact operation UUID, process owner epoch, selected root and
deadline to native adapters only. Failure must not return a freeze capability.
Adapters must honor the deadline; this registry cannot forcibly cancel an
uncooperative native call or detached process.

Each participant yields an owned, noncloneable `FrozenParticipant` capability.
It must revalidate its actual owner/root/session against the scope and yield only
its exact registered database connection. The legacy fallback requires actual
fully closed writer shutdown and retained original DB/sidecar identity, as
specified by `FrozenProfile`; no SDK reopen/migration is allowed. Capabilities
remain held across flush, inventory, backup/copy/hash verification and publication.
The final inventory is captured **after** flush, not updated silently from an
older reviewed file-stamp identity.

Every acquisition/callback result passes through one finalizer, including later
participant failure and deadline expiry after earlier capabilities were acquired.
It revalidates **all** acquired capabilities before any are released. Their
release must not initiate new writes. Panic unwinding quarantines admission
instead of assuming the uncompleted finalizer succeeded. Invalid acquired
owner/root/session proof quarantines
admission; it cannot be cleared by a boolean or automatic path rebind. Explicit
native recovery needs a freshly verified selected owner/registry. Ordinary
storage or deadline failure with otherwise valid owner proof releases normally.
Poisoned locks fail closed. All public errors are typed private-safe profile
errors; native status/UI must sanitize paths and participant diagnostics.

This RAII release policy is for preservation only. It must never wrap an
activation callback: resuming the old writers after a committed or uncertain
selector change is unsafe. `prepare/activate/rollback` remain unavailable. Their
next coordinator must persist candidate/import evidence, deny **both** owners
through commit/reconciliation and restart, read the exact durable selector, and
route startup before opening any SDK/plugin/provider/daemon owner. It must not
infer success or retry selection from a timeout. Root owns those composition/UI
hooks; actual participant adapters need separately assigned owner changes.

Two private synthetic fixtures pass:

- Missing participants and an issued effect prevent publication; new effects are
  denied while draining, only actual lease completion advances preservation,
  timeout publishes nothing, and all eight capabilities release afterward.
- Strict flush precedes final inventory; real SQLite backup is scoped through a
  participant. Replacing its retained root during backup prevents publication,
  preserves the original and quarantines later effect admission. A later
  participant that replaces the root and fails before returning its capability
  also causes earlier acquired owners to be checked and admission quarantined.

Observed gate (2026-10-08): a disposable harness imports this exact `mod.rs` under
`cfg(test)`, so its real child modules and fixtures compile without starting a
Tauri app. Dependencies are the existing locked `mdreader-profile`, rusqlite,
uuid and tempfile; the harness copied the workspace lock and audited every used
registry version/checksum against it. No repository dependency/lock change.
Warm benchmark target, `CARGO_BUILD_JOBS=2`, one assigned compiler process:

```sh
cargo test --locked --offline --lib
cargo clippy --locked --offline --all-targets -- -D warnings
```

Two tests passed in 0.09s (compile 3.09s); strict Clippy passed in 0.93s. No full
native/app/model build or installed-profile access ran. These fixtures isolate
orchestration/storage with private test adapters; they cannot establish real
writer quiescence, shipped startup or an installed migration gate. Next wire
real registered owners in coordinated slices and check the actual native app;
do not treat this unregistered module as the final deliverable.


## Strict JSON and output-only preservation checkpoint

The new `persistence/` modules implement the session/settings owner and its
scoped renderer flush bridge. They remain **unregistered**: startup must supply
one trusted profile `SourceRoot`, construct `JsonOwner`, register both bridge
participants, manage `commands::PersistenceState`, and register the five async
`native_profile_persistence_*` commands. Native glue needs `mdreader-profile`,
`serde_json/raw_value` and the already locked core/sha2/fs2/same-file dependencies.
This checkpoint does not modify app startup, installed profiles, or selectors.

Ordinary JSON read/write commands carry actual registry effect leases into
blocking work. One retained lifecycle lock binds the real directory and lock
object. Fresh renderer epochs reject queued packets from a preceding runtime;
strict monotonically increasing generations provide exact patch replay receipts.
Only after file and directory synchronization does the owner acknowledge a
SHA256 receipt. Uncertain publication quarantines the owner. Inputs are bounded
at 1 MiB per file, depth 32 and 32768 nodes, reject duplicate keys and unsupported
session schemas, and preserve unrecognized raw JSON values when merging known
fields. Original preflush bytes remain separate archival evidence, including
fields/rows no longer representable by the current renderer.

`ui/src/shell/persistence.ts` admits native saves only after an explicit,
successful current-state read with the registered owner/renderer identity.
Failed or mismatched reads may render defaults but cannot write those defaults;
only a later successful explicit load reopens admission. Reload attempts fence
older concurrent completions. This gate also protects preservation flushes.
Unsupported hosts retain their existing ordinary plugin-store adapter; their
installed-profile preservation remains unavailable. No fallback native writer is
used after failed native hydration. The UI must surface the hydration failure.

AppShell composition installs `profile-transition-bridge.ts` only after a real
save/discard preflight can retain the unsaved-editor capability. Its in-freeze
callback verifies that capability; it must not run an ordinary save after global
admission is denied. Native-issued request/window/operation/owner tokens scope
the latest session/settings flush. The registry drops all acquired participant
handles and explicitly resumes global admission before `post_resume` emits the
exact completed request notification. Quarantine, panic or uncertain publication
emits no resumption notice. The fixture synchronously acquires an ordinary
effect from the release callback to prove this ordering.

`mdreader-profile::bundle::stage_bundle` is an output-only outer envelope with
an unchanged verified current snapshot in `current/` and separately manifested
session/settings preflush originals. Absent and present-empty are distinct.
Each preimage is at most 1 MiB; total preimages are at most 2 MiB and count against
the overall archive limit. Fixed safe names, retained descriptors, hashes,
no-replace publication and directory synchronization protect the selected new
destination outside the source profile. No installed-profile sidecars or
activation are introduced. Future lifecycle receipts must bind the **outer**
bundle identity; the inner snapshot identity alone does not cover preimages.

Observed focused checks (2026-10-08, synthetic disposable roots only):

- Portable bundle fixture: 1 passed, 5 filtered; 0.12s tests, 2.41s compilation,
  warm `benchmark-target`, locked/offline, two jobs. Covered absent/empty,
  interruption, destination no-replace and tamper rejection.
- Existing exact native-module harness: imports this real `mod.rs` and child
  command/owner/bridge source, with `serde_json/raw_value` and actual Tauri/Specta
  types. Only the four new persistence fixtures ran: 4 passed, 2 filtered.
  Final run: 1.62s compilation and 0.17s tests; strict harness Clippy: 1.41s.
  Three cover real filesystem JSON validation/replay/reopen/identity; one covers
  the real registry/JSON/bundle orchestration with the other six participants
  synthetic. Final compile/test and strict harness Clippy use the compatible
  warm `CatDB/cargo-target-v1` cache, locked/offline, two jobs. All harness
  registry versions/checksums match the repository lock; repository lock unchanged.
- Frontend hydration regression observed RED (save incorrectly resolved after
  read failure), then both affected files passed all 9 tests. UI typecheck and
  scoped ESLint passed via existing cache executables; no dependency install.

The exact native harness references unregistered command symbols without
invoking a Tauri runtime; no global lint suppression is used. This is source and
seam evidence, not native startup, actual window lifecycle, installed migration,
SDK shutdown, daemon/vault quiescence, or activation/rollback acceptance. All
eight actual participant owners and durable startup/handoff remain required.
