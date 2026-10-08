# Held service preservation session

This native-only control slice retains the actual Runtime's daemon, account,
device and capture guards in the service process. It is not a whole-profile
preservation capability, backup API, selector operation or renderer command.
Provider OAuth/admission/vault effects still require independent custody.

`Client::connect` binds one explicitly selected root, its existing authenticated
endpoint inode/bytes and one service boot. It never calls `ensure_running`,
reloads another endpoint or rebinds after uncertainty. `Client::attempt` accepts
UUIDv4 operation/owner epochs and a duration of 1–30,000 ms. `begin`, `reconcile`,
`revalidate` and `release` use this same root/boot/scope. Dropping an attempt
sends nothing. A lost Begin reply is reconciled to the existing operation/lease;
a lost Release reply is reconciled to its terminal receipt.

The service admits one operation at a time. It keeps at most 64 terminal receipts
for its boot, without eviction that could allow an old Begin to mint a new lease.
After exhaustion, an explicitly coordinated restart is needed; clients do not
restart the daemon automatically. Control request acceptance/replies are bounded
to 8 KiB; the inherited one-shot transport first allocates/parses at most its
2 MiB request limit before recognizing a control request. Selected paths to 4 KiB/64 components, and endpoint reads to 4 KiB. The ordinary
transport's bounded authenticated handler limit is unchanged.

Freeze order is actual daemon deny/drain, account actor/task/vault drain, device
lease drain, then cross-process spool lock acquisition. No owner or control lock
spans these waits. Partial acquisition retains every acquired guard and marks
the session quarantined. Deadline expiry, lost network replies, root/boot/scope
mismatch and client destruction cannot release held guards or claim readiness.
Explicit Release revalidates all guards, relinquishes independent producers,
checks their actual admission state, and releases the daemon barrier last.
Account/device observers record the exact operation/epoch released; capture and
daemon admission must actually succeed. An uncertain release retains remaining
barriers and reports quarantine, never Released. This does not promise atomic
resumption across independent owners after a poisoned gate or failed release.

No original SQLite file is opened by this client or control seam. Durable remote
inventory will declare `reader-v1/library.sqlite`; the attached `index.sqlite`
and its recovery files are live rebuildable objects and must be protected from
Raw inventory separately. Only a later reviewed server-side snapshot operation
may borrow the actual guarded owner. Unsupported hosts and standalone/mobile
constructors cannot issue this control capability.

## Checkpoint evidence

Three fixtures use the actual Runtime, real SQLite owners and the actual
authenticated TCP dispatch in synthetic temporary profiles. They cover uncertain
Begin/Release reconciliation and ordinary-request denial, stale
boot/epoch/lease/endpoint fencing, and expiry retaining guards. Accounts are
explicitly unconfigured in this fixture; this does not establish live
identity/vault custody, native startup registration or all-eight readiness.

All commands ran locked/offline with `CARGO_BUILD_JOBS=2` and the existing
`CARGO_TARGET_DIR=/Users/mlong/Library/Caches/MDReader/benchmark-target`:

```sh
cargo test --locked --offline -p mdreader-service --features sync-native \
  --lib preservation::client::tests
cargo clippy --locked --offline -p mdreader-service --all-targets -- -D warnings
cargo clippy --locked --offline -p mdreader-service --features sync-native \
  --all-targets -- -D warnings
```

Initial tests: 3/3 GREEN, 12.55 s incremental compile / 0.59 s execution.
Meaningful RED: temporarily route actual Runtime `ListSources` around its real
ordinary admission gate. The real TCP fixture compiled (2.61 s) and failed its
ordinary-request denial assertion (0.08 s). An initial temporary mutation had a
MutexGuard dereference compile error; that is not counted as behavioral RED.
Restored the exact Runtime bytes, SHA256
`8ac8c21a649954e4f7c9e545b9914a847d4badd7acf9f35b4b26da571303b666`;
restored 3/3 GREEN, 2.19 s incremental compile / 0.56 s execution. Strict
all-target Clippy GREEN for default (5.17 s) and `sync-native` (7.42 s).
Test artifact: warm target `debug/deps/mdreader_service-7f0e66053bcfb897`.

No app/model/package/private-profile acceptance is claimed. Next required work
is the reviewed server-side backup operation and actual native owner composition,
not another detached readiness certificate.
