# Native account owner composition

This module is feature-gated by `sync-native`. It composes accepted native
OIDC/PKCE, nonce enrollment, OS-vault custody, OOB pairing, recovery, bounded
transport and the same reader owner's incoming projection. It is **not wired
into service startup/dispatch or the product Settings panel** in this checkpoint.
No default registration, renderer token/key input or second database is added.

## Native composition seam

Construct **one** `Arc<accounts::Coordinator>` per live `Arc<Mutex<ReaderStore>>`.
Supply `NativeSetup` with reviewed native registration/factory, exact registered
loopback redirect, matching transport endpoint and a native browser adapter.
The controller comes from that exact factory; transport and incoming projection
share its live authority. Supply native-owned document/asset cache roots, never
renderer paths. `None` setup is explicitly unconfigured; offline reading remains
available and no OAuth/vault call is made during construction.

`NativeRecovery` is a native file/secure-input/output adapter, not a generic RPC.
It must bound a selected encrypted file before allocation (33792 bytes), keep
the independent recovery secret separate from the file, and cancel pending
native dialogs when its future is dropped. Its file/secret inputs and
`RecoveryExport` must never be logged, serialized to the renderer or persisted
outside the accepted native vault lifecycle. Mobile authentication and mobile
secure custody remain unsupported by the existing desktop factory.

Proposed thin future service commands (not dispatched here):

- `status()` → redacted configuration/custody/connected/latest-job state.
- `begin(AccountIntent)` → opaque UUID immediately; `operation(UUID)` polls.
- `cancel(UUID)` cancels only that running job; `disconnect()` signs out locally.
- `apply_prepared(PreparedIncoming)` is a **native blocking-worker** seam, never
  renderer data. It delegates to the selected existing atomic owner projection.

The renderer's standard Settings actions provide explicit product consent for
sign-in/bootstrap/join/recover; there are no additional routine native approval
dialogs. Recovery export and approval/acceptance of a full OOB device packet
remain separate explicit actions. The panel must never automatically invoke
approval based on the server roster, infer trust from a short code, or claim a
local request packet proves server acceptance. Public OOB packet text is the
only transfer artifact in a job outcome (at most 8192 bytes).

## Bounded ownership and cancellation

One native actor thread/current-thread Tokio runtime owns the factory and
private pending custody. Queue capacity is one, only one running job is accepted,
and history retains the latest eight jobs. Callback phases each allow at most
five minutes; native recovery dialogs allow five minutes; an entire job has an
eleven-minute future bound. Cancellation/timeouts still retain and drain actual
blocking owner work before releasing the running slot: an unresponsive filesystem
can therefore prevent settlement, and preservation must time out without proof.
Transport retains its existing 60-second bound and uses
five outgoing operations, one five-item root page and 8 MiB per foreground root run.
Dependency hydration is separately bounded to 32 MiB and at most five contiguous
owner projections per explicit action; each chunk request is at most 512 KiB.
No automatic background/reconnect loop is added.

All owner/std mutex guards are short and synchronous. None crosses network,
browser or OS-vault awaits. Cancel/sign-out advances the job epoch and invalidates
the exact controller, dropping the one in-flight operation future. The running
slot remains occupied until that drop and all retained blocking-worker handles
settle. Already-issued OS writes may outlive their original receiver, so a native
preservation freeze also waits for an actual FIFO vault barrier. Late completion, stale cancel and
old job IDs cannot publish a replacement status or sign out a newer job. Existing
authority hooks still invalidate only the rejected lease on remote 401/403.

## Durable state and ambiguity

The owner borrows its own SQLite connection to persist one immutable remote
library UUID and an opaque OS-vault reference. No token, private key, account
identity, recovery secret or decrypted incoming body enters these tables.
The reference is committed **before** OS-vault creation. A failed/uncertain
create retains the exact in-memory private record for `retry_custody_write`.
A restart with a retained reference but absent vault material fails closed;
it cannot generate replacement keys. Switching/deleting/rebinding accounts and
repairing missing vault material need a future explicit preservation workflow.

Bootstrap/join/recover use fresh verified ordinary login followed by the
persisted nonce-bound login. Uncertain or expired dispatch is not silently
replayed: `reconcile_enrollment` probes the same enrolled material;
`renew_enrollment` explicitly invokes existing same-material renewal. A denied
GET cannot distinguish never-enrolled from revoked and never establishes
eligibility or authorizes regeneration. Original intent history remains in the
vault. Uncertain approval has its own exact retry/handoff actions. Paired devices
without the recovery anchor cannot export an invented replacement anchor.

`sync_now` first transfers/stages authenticated encrypted roots, then authenticates
their exact content/resource graph using native custody-held keys. It downloads
only bounded SHA256 dependencies, reassembles/validates original Markdown and
assets on retained blocking work, and invokes the existing selected owner
projection for each contiguous revision. Only that atomic owner transaction may
advance **applied**; a final transport pass acknowledges the durable applied
cursor. **Received**, **applied** and **acknowledged** remain independent.

Conflicts, dirty local state, unknown tombstones, unsupported block positions and
stale positions stop this action at the last safe cursor with a typed `pending`
sequence/reason. Invalid/missing/tampered graphs fail without projecting their
root or acknowledging it. There is no placeholder advancement, silent conflict
resolution, automatic reauthorization, position approximation or background loop.
Native authority remains private; no key/token getter or renderer preparation API
is introduced. The explicit outgoing native-document association/dependency
outbox and actual product reader discovery are coordinated follow-on slices.

## Native preservation seam (not a backup receipt)

`Arc<Coordinator>::freeze(operation, owner_epoch, deadline)` denies new actions,
external projections and disconnect, while allowing already-issued actor work to
settle naturally. It does **not** sign out or cancel the profile. The deadline is
positive and at most 30 seconds. After running/projection leases drain, an actor
message waits for the real FIFO vault worker, including mutations whose original
receiver timed out. Success returns a nonserializable `FrozenAccounts` guard
bound to this exact coordinator/operation/epoch/deadline; revalidation checks all
of them and actual quiescence. Dropping the exact guard reopens admission. Timeout
returns no proof and leaves the original session/intents intact.

This guard does not expose a connection or claim all reader participants/root
files are frozen. A native service preservation adapter must retain it in the
actual owning process, bind the actual root plus every other participant, and
stream/write only a separately validated selected export target. It cannot ship
an Arc/callback across IPC, treat disconnected status as quiescence, or accept a
renderer-supplied boolean as a backup capability. That process/backup adapter is
a separate coordinated integration gate.

## Focused local evidence (2026-10-08)

All commands used `CARGO_BUILD_JOBS=2`, locked/offline dependencies and the sole
coordinated check slot with the existing compatible `benchmark-target` cache.

```sh
cargo test --locked --offline -p mdreader-service --features sync-native accounts::tests:: -- --nocapture
cargo test --locked --offline -p mdreader-sync --features native-authority --lib authority:: -- --nocapture
cargo test --locked --offline -p mdreader-sync --features native-authority --lib authority::tests::pending_key_custody_reconciles_timeout_and_reopens_same_material -- --exact
cargo clippy --locked --offline -p mdreader-service --features sync-native --all-targets -- -D warnings
```

The first command initially produced **2 PASS / 1 FAIL** (0.37s): Serde accepted
an extra synthetic `access_token` on an internally tagged unit action even with
`deny_unknown_fields`. Empty struct action variants now enforce the closed input
contract with the same JSON shape. The affected three tests then passed (0.33s):
retained-reference reopen/no regrant/future-schema preservation; actual native
callback cancellation/owner freedom/stale-job fencing without credentials;
unconfigured local operation/secret DTO rejection/bounded history.

Affected authority tests passed **23**, with three scheduled HTTPS/JOSE cases
explicitly ignored by that ordinary suite (1.21s; those existing release cases
were not rerun for this composition-only checkpoint). The extended existing
custody-reopen test passed (0.36s), checking safe summary state, absent material
and rejection of a different library. Strict service all-target Clippy passed
(12.28s). Initial service compilation took 24.57s, fix compilation 8.69s;
authority compilation 5.24s and the extended test 3.24s. Formatting and diff
checks passed. No lockfile/dependency/protocol changes, app packaging or deployment.

These tests isolate storage/cancellation and accepted authority behavior; they
do **not** establish a complete coordinator login/bootstrap/pair/recovery flow.
Actual configured provider login, product RPC, native file/dialog adapters,
startup wiring, typed sync preparation/pump and mobile acceptance remain separate.

## Typed pump/preservation checkpoint evidence (2026-10-08)

With the same sole slot, jobs2/locked/offline and warm cache:

```sh
cargo test --locked --offline -p mdreader-service --features sync-native --lib accounts::tests:: -- --nocapture
cargo test --locked --offline -p mdreader-service --features sync-native --lib sync::incoming::tests::staged_graph_requires_verified_bodies_before_atomic_owner_projection -- --exact --nocapture
cargo test --locked --offline -p mdreader-sync --features native-authority --test incoming -- --nocapture
cargo test --locked --offline -p mdreader-sync --features native-authority --lib authority::tests::pending_key_custody_reconciles_timeout_and_reopens_same_material -- --exact --nocapture
MDREADER_SYNC_WEB_WORKTREE=/ABS/REVIEWED_WEB_WORKTREE cargo test --locked --offline -p mdreader-sync --features native-authority --lib transport::tests::actual_worker_immutable_retry_scoped_receipts_staging_and_applied_only_ack -- --exact --ignored --nocapture
cargo clippy --locked --offline -p mdreader-service --features sync-native --all-targets -- -D warnings
cargo clippy --locked --offline -p mdreader-sync --features native-authority --all-targets -- -D warnings
```

Results: accounts **3 PASS** (8.74s compilation, 0.32s tests); selected SQLite
staged-graph case **1 PASS** (0.06s); incoming crypto tests **7 PASS** (3.07s
compilation, 0.17s tests); extended vault timeout/reopen **1 PASS** (2.44s
compilation, 0.27s tests). The graph case reopens the actual owner, discovers
only authenticated hashes, rejects missing/tampered bodies, then verifies exact
Markdown survives atomic publication/reopen with applied=1 and acknowledged=0.
The callback case proves freeze timeout neither drops the waiting browser nor
claims logout, and a successful exact-scoped freeze blocks new sign-in and
sign-out until its guard drops. The vault case verifies the FIFO barrier returns
only after the original timed-out OS mutation actually populated the native vault.

The **existing actual local HTTPS Worker/SQLite/R2 fixture** passed once (2.76s
compilation, 1.47s test). Its new resource-download assertions verify exact stored
ciphertext, aggregate byte-bound rejection, duplicate dependency rejection and
unchanged received/applied/ack cursors. These assertions complement the real
SQLite typed-graph test; they do not claim an end-to-end account actor/pump run.

Temporarily removing only `begin`'s frozen-admission guard produced **RED** at
an attempted sign-in during a held successful freeze (0.27s); exact source was
restored byte-for-byte and the focused regression returned **GREEN** (0.28s).
Strict service and sync all-target Clippy then passed (5.13s and 2.27s).
Initially shared-cache Clippy consumed old cross-worktree sync dependency metadata
and reported missing new APIs despite successful test builds. Touching the owned
sync crate-root timestamp forced correct dependency recompilation; no code
workaround, cache deletion, dependency or lock changes were made. Formatting and
diff checks passed. No app packaging, deployment, provider login or private data.

Remaining acceptance: actual account-actor typed-pump integration and cancellation
of a running owner projection; cross-process preservation/backup capability;
explicit outgoing associations plus immutable encrypted dependency receipts;
same-owner product catalog/asset read composition; real provider/user login and
mobile lifecycle. Existing incoming owner regressions separately cover
replacement, dirty-state CAS, pins, exact line positions, conflicts and deletions.
Block/pixel positions remain a typed pending state rather than approximated lines.
