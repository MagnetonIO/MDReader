# Capture platform extraction receipt

Scope: synthetic-only capture crate verification on macOS, isolated branch
`feat/reader-capture-platform`, base `21bc807`. Pinned Rust toolchain: 1.94.0.
No private profiles, agent histories, credentials, installation, model inference,
network providers or application integration were exercised.

## Observed checks

All commands ran serially with two Cargo jobs against an existing warm cache:

```sh
CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/Users/mlong/Library/Caches/MDReader/benchmark-target cargo test --locked --offline -p mdreader-capture
CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/Users/mlong/Library/Caches/MDReader/benchmark-target cargo clippy --locked --offline -p mdreader-capture --all-targets -- -D warnings
CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/Users/mlong/Library/Caches/MDReader/benchmark-target cargo check --locked --offline -p mdreader-capture --all-targets --target x86_64-pc-windows-gnu --features blake3/pure
CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/Users/mlong/Library/Caches/MDReader/benchmark-target cargo clippy --locked --offline -p mdreader-capture --all-targets --target x86_64-pc-windows-gnu --features blake3/pure -- -D warnings
rustfmt --edition 2024 --check crates/mdreader-capture/src/lib.rs
git diff --check
```

- Capture suite: 25 passed, zero failed (1 library, 2 backfill, 15 capture,
  7 subprocess tests). Build 2.26 seconds; test groups approximately 1.60 seconds.
- Host strict all-target Clippy: passed, 0.85 seconds.
- Windows GNU all-target check: passed, final cached check 0.94 seconds.
- Windows strict all-target Clippy: passed, final cached check 0.77 seconds.
- Formatting and whitespace checks: passed.

The initial Windows check caught a misplaced validation call; Windows strict
Clippy caught one needless borrow. Both were corrected. The host suite initially
reported one unused extraction import, removed before strict checking. These are
compile/lint observations, not Windows behavioral RED/GREEN evidence.

`blake3/pure` was supplied only for cross-checking: the installed Windows Rust
standard library is available, but this host has no MinGW compiler/linker. It does
not change production dependency features or demonstrate a linked Windows binary.
The three Windows-only tests compile but were not executed. They cover the public
fail-before-mutation gate, inherited-DACL/hardlink rejection and nonblocking lock
contention, and pinned namespace/handle-relative replacement with bounded exact
body reads.

## Boundary and remaining acceptance

Unix durable records, JSON, public calls, queue/receipt ordering and filesystem
operations are retained. Policy, projection encoding, queue operations and
filesystem operations are separated into cohesive modules. Existing subprocess
crash/restart, duplicate/receipt corruption, opt-in and nonblocking-lock tests pass.

Windows `Spool::open` still returns `UnsupportedPlatform` before namespace
creation. The isolated Windows primitives do real file flushing/write-through
opens; directory syncing returns an explicit error, never a successful no-op.
The missing production requirement is a defensible namespace durability barrier
covering publication, receipt creation and pending-event deletion. See the
[documented Windows contract and primary sources](../README.md#durability-budgets-and-incomplete-host-gates).
No Windows activation/runtime support is claimed.

Real Windows ACL/reparse/junction/concurrent replacement, process-crash,
power-loss, hook latency and end-to-end service acceptance remain unrun. Local
fixed NTFS is the only filesystem considered by the prototype primitives; remote,
removable and other filesystems are rejected. Runtime support must not be enabled
merely because this cross-check passed.

Dependency delta: Windows-target-only `windows-sys =0.61.2` with seven narrow
Win32 feature groups, and one existing-package capture dependency edge in
`Cargo.lock`. No package version/checksum changes, core dependency or new global
runtime feature switch.
