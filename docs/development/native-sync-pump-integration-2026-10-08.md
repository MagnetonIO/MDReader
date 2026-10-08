# Native sync pump integration checkpoint

Coordinator-owned minor branch `feat/native-sync-pump-integration`, isolated in
`MDReader-sync-pump-integration` from native integration `2632677`. It composes
typed pump/preservation `a2d17f8` and shared asset runtime `0c4e19f`; merge
checkpoint `da088b9` contains both and the intervening native Settings changes.
Destination is `dev-reader-next`, with normal minor worktree cleanup after merge.

## Behavior accepted at this checkpoint

An explicit native sync action stages authenticated encrypted roots, hydrates
only their authenticated bounded dependencies, and uses the existing selected
SQLite owner to publish at most five contiguous revisions. Received, applied
and acknowledged cursors remain independent. Pending/conflicting revisions stop
projection; only durable applied state is acknowledged. The actor retains and
drains blocking jobs across cancellation before releasing its running slot.

The native preservation guard denies new account/projection work, drains issued
work and the actual vault FIFO, and binds the exact coordinator, operation,
owner epoch and deadline. It is one participant's capability, not a profile-wide
backup proof. The new composition constructor accepts the exact shared
`AssetRuntime` needed by subsequent same-owner document/asset discovery.

## Combined-tree verification

All checks used the sole heavy slot, two Cargo jobs, locked/offline dependencies
and the existing `benchmark-target` cache. No installation, cold build, model
benchmark, full app build or lockfile change occurred.

```sh
cargo test --locked --offline -p mdreader-service --features sync-native --lib accounts::tests
```

Three tests passed (12.72 s compilation, 0.30 s execution). The existing compiled
service test executable then ran only `sync::incoming::tests --test-threads=1`:
ten passed in 0.48 s, covering staged graph verification and real SQLite/filesystem
publication, reopen, replacement, pin/position, tombstones and migration.

```sh
cargo clippy --locked --offline -p mdreader-service --features sync-native --all-targets -- -D warnings
```

Strict Clippy passed in 6.70 s. The source review included actor cancellation,
retained blocking handles, account freeze, transport dependency bounds and
shared-runtime construction. The underlying pump's separate local HTTPS Worker
and authority checks are recorded in `crates/mdreader-service/src/accounts/README.md`.

## Remaining acceptance

These thirteen combined checks establish owner/storage and bounded adapter
behavior; they do not exercise a complete account-actor login/recovery-to-pump
flow. That actual local HTTPS fixture, including cancellation during projection,
is in progress separately. Outgoing immutable dependency graphs/associations,
same-owner product document/asset discovery, all-owner preservation/backup,
live issuer/provider acceptance and desktop/mobile packaging remain open. No
installed library, user credential, cloud resource or deployment was changed.
