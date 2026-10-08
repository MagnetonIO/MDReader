# Native capture and outgoing composition

Root owns `feat/native-capture-composition`, based on integration `25cf40d`.
Capture `302b0e141e6b1dc70283826fa5fbf7d749d058c2` and outgoing graph foundation
`e02b8c95a3d3de9233e0d86dc3cb16658706faee` are normally merged. Combined source
commit `ee2e2207c728c12b9a8d8113479ac6b85cdc4635`, source tree
`aed63269b0334e6fe904a52563c7263e290ef5c2`.

## Behavior and boundaries

The actual Runtime capture worker and configure/backfill/integration requests
share the effect gate. Native freeze drains issued effects and retains the real
cross-process spool lock and namespace identities, preserving queued, interrupted
and uncertain output. The device reader, shared image runtime and account actor
remain composed with this Runtime. This is one participant, not an installed
profile backup or all-writer freeze.

Native custody can seal bounded Markdown/image graphs. The sync store admits
immutable ciphertext and nonce reservations on the caller's existing transaction;
foreground transport retains scoped dependency receipts, resumes exact bytes and
publishes the root only after dependencies. No protocol, dependency, lockfile or
deployment changes. Product selection, native association and same-document echo
remain gated; no upload RPC/UI is enabled here.

The documented capture constructor identity gap remains: SQLite's filename plus
a newly retained named file cannot prove SQLite originally opened that inode.
Retained SQLite-open identity and the outgoing core read-only current fence are
separate work, excluded from this checkpoint.

## Focused combined evidence

One heavy command at a time used the existing warm
`/Users/mlong/Library/Caches/MDReader/benchmark-target`, `CARGO_BUILD_JOBS=2`,
`--locked --offline`. No clean, new dependency tree, model or app package build.

```sh
cargo test --locked --offline -p mdreader-service --features sync-native --lib capture::admission::tests:: -- --nocapture
cargo clippy --locked --offline -p mdreader-service -p mdreader-capture --all-targets -- -D warnings
cargo clippy --locked --offline -p mdreader-service --features sync-native --all-targets -- -D warnings
cargo test --locked --offline -p mdreader-capture --test hook_process preservation_lock_fences_real_hooks_and_keeps_interrupted_receipt_bytes -- --exact --nocapture
```

Service compile **18.06s** yielded the fresh artifact
`debug/deps/mdreader_service-7f0e66053bcfb897`. Direct runs of that artifact:

- Capture admission: **2 pass / 0.07s**.
- `runtime::capture_admission_tests::actual_runtime_capture_and_integration_paths_are_denied_while_frozen`:
  **1 pass / 0.06s**. An initial wrong module filter matched zero tests; the
  actual artifact list identified this exact name. Zero tests are not evidence.
- Device owner/catalog/shared assets: **3 pass / 0.12s**.
- Account lifecycle: **3 pass / 0.34s**.
- `accounts::acceptance::actual_actor_pkce_recovery_pump_and_retained_projection_cancel`:
  **1 pass / 2.07s**, isolated child **1.54s**, with
  `MDREADER_SYNC_WEB_WORKTREE` set to the reviewed companion integration checkout
  and its existing local Worker bundle. Synthetic identity/TLS/SQLite/R2 only;
  no OS keychain or live Auth0 enrollment. See the
  [actor contract](../../crates/mdreader-service/src/accounts/acceptance/README.md).

Strict all-target Clippy passed default service/capture in **6.00s** and shipping
`sync-native` service in **6.52s**, without suppressions. The real hook subprocess
case passed **1 / 1.10s**, compilation **2.14s**. Total: **11 focused cases** across
two artifacts, not a full matrix. Outgoing's real SQLite/HTTPS retry checks are
recorded in its [receipt](../../crates/mdreader-sync/src/outgoing/README.md), not
claimed as rerun here. No frontend or generated native contract changed.

## Remaining release work

SQLite-open identity, ReaderDaemon and other preservation owners; installed
export/migration/selector/rollback; selected outgoing association and exact echo;
shipping account/recovery UI and real vault/login; provider OAuth/offline
acceptance; CatDB removal; physical mobile/browser/packaged upgrade checks;
fresh-clone startup and final release packaging remain unfinished. No installed
library, private agent history or cloud deployment changed.
