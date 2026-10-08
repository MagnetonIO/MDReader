# Actual native account actor acceptance

This is one explicit local integration case, not live Auth0 or installed-profile
acceptance. It composes the production account actor, OIDC signature/issuer/client
and nonce checks, code + S256 PKCE, bound loopback listener, standard encrypted
recovery import, native custody codec/admission, device proof, HTTPS transport,
Worker SQLite/R2 and selected incoming owner transaction.

The public synthetic browser peer uses normal Worker challenge/bootstrap,
scoped immutable uploads and commit routes. It cannot write Worker tables. Its
published test library key and independent recovery secret are fixture material;
native signing/wrapping keys are generated and retained through the real factory.
There is no native raw-key getter, injected authority or permissive TLS client.
A child process limits TLS trust to its temporary CA through `SSL_CERT_FILE` and
an empty `SSL_CERT_DIR`. Its keyring credential builder is process-memory-only;
production `SyncKeyringVault` still encodes, validates and reopens private records.
The host OS keychain and trust store are never modified.

The private `cfg(test)` incoming barrier runs only after real chunk preparation,
file materialization and raster validation, before the existing final selected
commit. It can pause execution, but cannot inject inputs, keys, authority or
substitute a result. It adds no shipping branch or changes to production ordering.

## Reproduce

Use the reviewed companion web checkout with its frozen existing dependencies:

```sh
# In that web checkout; local bundle only, no deployment.
pnpm exec wrangler deploy --dry-run --config wrangler.sync.jsonc --outdir .worker-build/sync

# In the native feature checkout; sole coordinated check slot, warm cache.
export CARGO_BUILD_JOBS=2
export CARGO_TARGET_DIR=/ABS/EXISTING_COMPATIBLE_CARGO_CACHE
export MDREADER_SYNC_WEB_WORKTREE=/ABS/REVIEWED_WEB_WORKTREE
cargo test --locked --offline -p mdreader-service --features sync-native --lib accounts::acceptance::actual_actor_pkce_recovery_pump_and_retained_projection_cancel -- --exact --ignored --nocapture
cargo test --locked --offline -p mdreader-service --features sync-native --lib accounts::tests::
cargo clippy --locked --offline -p mdreader-service --features sync-native --all-targets -- -D warnings
```

Node, the companion Miniflare/jose packages, OpenSSL and curl must be available.
No shell executes callback URLs or recovery material; test HTTP configuration
travels on subprocess stdin. The Worker and disposable profiles are stopped and
removed after the case. The test is ignored by ordinary unit runs because it
requires this explicit external bundle; its scheduled execution is mandatory
for this checkpoint.

## Observed 2026-10-08

Reviewed clean web source was `02b66e13739ede471a17aecfb152aab04bbd612f`.
The Wrangler dry-run completed in 0.83s. An initial fixture execution failed
before any actor assertion: Miniflare 5's converted `scriptPath` loader rejected
the bundle with an internal startup error, including after the bundle refresh.
Minimal SQLite/R2 and the exact full bundle supplied as `script` both started.
The fixture now reads those exact bundle bytes directly, without transforming
source or changing Worker behavior. This was a fixture startup correction.

The exact actor case passed initially in 2.01s (inner child 1.52s, initial Rust
compile 6.40s). It verifies:

- Actual recovery enrollment and `SyncNow` publish exact BOM/CRLF Markdown and
  a fully decoded valid PNG through the same reader owner/shared asset runtime.
- A subsequent encrypted user-state revision sets the pin and exact line 2.
- A second real recovered device pauses its actual blocking materialization;
  cancellation makes status Cancelled while admission stays Busy and preservation
  times out without a false settled proof. SQLite is not locked during the pause.
- Releasing that real task leaves received=2, applied=0 and acknowledged=0,
  with no visible document. Fresh sign-in preserves the durable old intent and
  reports explicit `reauthorization_required`, rather than silently restarting it.
- A normal peer content tombstone retires the first owner's document; applied
  and acknowledged advance together only after the actual owner retirement.

Removing only `Actor::settle_blocking().await` produced meaningful **RED** in
1.72s: freeze incorrectly succeeded while the real materialization was paused.
Exact actor source was restored byte-for-byte. The restored case passed in
**1.90s** (inner 1.44s, compilation 2.40s); the existing account module passed
**3 tests in 0.30s**; strict service `sync-native` all-target Clippy passed in
**6.13s**. Formatting and diff checks passed. Resolution added only the direct
service dev-test keyring dependency edge; every package version/source/checksum
and package set remained unchanged.

These results do not claim product recovery dialogs, selected outgoing uploads,
live issuer login, mobile custody, cross-process preservation backup or installed
application acceptance. The separate outgoing/core-fence drafts are not part of
this checkpoint. There was no deployment, account mutation or private-data access.

## Selected native actor checkpoint (2026-10-08)

The same scheduled case now exercises the public native-only
`Coordinator::begin_selected_sync(document, operation)`. A private actor action
retains the actual projection lease and blocking JoinHandle while calling the
production IncomingCoordinator queue seam, then uses the existing authenticated
foreground pump. AccountIntent, renderer RPC, DTOs and UI are unchanged.

The fixture imports a second account-free native Markdown document and PNG through
core's real APIs under the actual configured owner roots. Its raw bytes equal the
peer document deliberately, proving that equal native hashes do not alias UUID
ownership. The real selected NativeAuthority encrypts and uploads both dependencies
and the root; strict scoped receipts and a commit receipt permit same-document
echo. Native document/source/asset UUIDs, BOM/CRLF, pin and exact line survive;
applied and acknowledged reach 3 without a duplicate import.

The second real recovered account pauses actual selected queue work after sealing
and before admission. Cancellation retains the real blocking task; new admission
stays Busy, freeze times out without proof, and SQLite remains available. Releasing
the task leaves no graph/nonce/association admission. A subsequent native login
and queue pauses its actual HTTPS upload after durable admission; cancelling that
job preserves exact encrypted root bytes, nonce reservations and the queued
association. No implicit reauthorization or deletion is performed. The preceding
incoming cancellation/reauthorization and later peer tombstone assertions remain.

Using jobs=2, `--locked --offline`, feature `sync-native` and the existing warm
benchmark target, the exact scheduled case passed first in 2.27s (child1.75s,
compile5.34s). This is **one** test case executed by a TLS-scoped child, not two
independent tests. Temporarily bypassing only post-materialization authority/
selection validation produced the expected RED in 1.98s (child1.55s, compile2.38s):
the cancelled queue incorrectly admitted one root, three nonce reservations and
one association instead of zero. Exact source was restored byte-for-byte. The
restored case passed in 2.15s (child1.73s, compile1.93s); the affected three account
unit cases passed in 0.31s; strict all-target service/sync-native Clippy passed in
3.08s. No compile/test failure occurred outside that intentional mutation.

Explicit companion web source was `8be2b785c1494703505cd6cab2c9541cb7ed5f06`;
the unchanged production Worker dry-run bundle SHA256 was
`fb22baded8f9b9db8aaa44962bb021962c4f2dd9333780250825ce7950c98a22`.
There was no web rebuild/deployment, bridge/protocol/dependency/lock change,
model run, private-profile or live-account access.

This establishes synthetic authenticated native actor queue/transport/self-echo
and retained cancellation boundaries. It does not establish actual OS-keychain
custody (the fixture retains its test-only keyring backend), live Auth0, mobile,
product QueueDocument RPC/UI or packaging. Old queued intent reauthorization,
outgoing user-state/deletions and cleanup remain gated. Repeated full graph proofs
under owner access are still a responsiveness follow-up; this checkpoint does
not intermingle the proposed transaction-bound proof reuse refactor.
