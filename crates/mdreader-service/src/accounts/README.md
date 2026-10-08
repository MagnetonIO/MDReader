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
eleven-minute bound. Transport retains its existing 60-second bound and uses
five outgoing operations, two five-item pages and 8 MiB per foreground run.
No automatic background/reconnect loop is added.

All owner/std mutex guards are short and synchronous. None crosses network,
browser or OS-vault awaits. Cancel/sign-out advances the job epoch and invalidates
the exact controller, dropping the one in-flight operation future. The running
slot remains occupied until that drop settles. Late completion, stale cancel and
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

`sync_now` stages authenticated encrypted roots and reports **received**, **applied**
and **acknowledged** independently. It does not decrypt/prepare chunk graphs or
pretend root staging applied selected Markdown/assets/user state. A native
typed preparation/transport pump, explicit library selection and outgoing local
selection orchestration remain product gates. The existing `apply_prepared`
owner transaction alone may publish and advance applied; no placeholder cursor
advance is added here.

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
