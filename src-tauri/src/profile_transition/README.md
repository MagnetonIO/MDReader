# Native profile writer admission (unregistered)

Based on accepted integration `e766942`. The portable foundation and the two
exact-native-module fixtures pass; strict harness Clippy passes. The module is
not registered in app startup and no existing command,
renderer, SDK, daemon or hook is protected by it. No installed profile was read
or changed. No production activation/rollback acceptance is claimed.

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
