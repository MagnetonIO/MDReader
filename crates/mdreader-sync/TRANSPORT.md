# Native transport foundation — production activation remains gated

The optional `mdreader-sync/transport` feature implements bounded HTTPS Worker
upload/commit/change/root-download/ack orchestration. The optional service
`sync-transport` feature supplies `TransportOwner` over the existing
`Mutex<ReaderStore>` and an explicit native active-scope binding barrier. Neither
feature is enabled by default. There is no product RPC, scheduler, sign-in UI or enabled production authority. The separate
[default-off native authority foundation](AUTHORITY.md) adds reviewed-configuration
OIDC/signing/custody code; activation and real-host acceptance remain gated.

`Authority` is a trusted native capability, not a JSON DTO. Its factory must
verify standard OIDC issuer/audience/nonce/PKCE/state, derive account scope from
verified issuer and subject, complete server device enrollment, and retain bearer
and ES256 signing capability/library keys entirely in native custody. An authority
generation changes on logout, key/session replacement or revocation, even if the
account/library/device tuple returns to the same value. Binding changes must also
invalidate the owner scope while holding its binding write barrier. The injected
capability signs the exact method/HTTPS URL including query, bearer SHA256 and
exact body SHA256 with fresh proof time/jti. Headers have sensitive formatting,
no getters, no Debug and no serialization; HTTP errors never retain raw URLs,
response error bodies, reqwest errors, custom SQLite/owner error text or tokens in diagnostics.
Transport itself delivers every remote 401/403 through `Authority::rejected`; a
native authority invalidates its own lease and owner binding, including when the
caller directly uses `Transport::run`. A stale response cannot invalidate a newer
session with the same account/library/device scope. HTTP/header
buffers necessarily hold transient copies; zeroization is best effort, not a
claim that transport allocations are comprehensively wiped.

`Transport::new` accepts an HTTPS origin only, rejecting credentials, non-root
paths, query and fragment. It constructs request paths/query itself, validates
same origin and exact path, disables redirects/environment proxies/referer,
automatic retries and response decompression, and uses native TLS roots. The
additional synthetic CA constructor is private; product callers cannot supply an
insecure client or bypass HTTPS. Each HTTP request is at most 15 seconds (5 second
connect/read bounds); an entire run is at most 60 seconds. Default budgets are
10 outgoing operations, 5 change pages of 10 entries, and 8 MiB total uploaded +
downloaded body bytes; caps are 20 operations, 10 pages of 20 entries and 32 MiB.
Envelope, receipt and page bounds are 2 MiB, 4 KiB and 256 KiB. Exhaustion returns
a typed error with previously durable progress intact.

One in-process gate covers each account/library/device scope across transport
instances. No spawned task owns network work: cancellation drops the active
request future and scope guard. The server may finish an already authorized
request after cancellation, so replay must remain idempotent. Every request
validates native authority/generation and the owner binding before network, then
again after receiving the bounded response. The owner connection is borrowed
only inside short synchronous closures; no owner mutex or SQLite transaction is
held across an HTTP await. The gate is not a cross-process owner lock; the reader
owner still supplies its existing process/library exclusivity.

## Durable boundary

1. Selected work is already sealed/enqueued in the owner's SQLite transaction.
   HTTP copies the saved ciphertext and operation; it never regenerates a nonce,
   revision or operation identity on retry.
2. Every nondeleted operation uploads saved bytes with exactly
   `?operationId=<id>&libraryId=<id>`. `mark_uploaded_scoped` requires a strict v2
   receipt binding verified account, library, enrolled device, operation, immutable
   blob SHA256 and exact byte count. Legacy `mark_uploaded` remains compatible,
   but the new transport always reuploads, including entries already marked
   uploaded through a legacy path, so a weaker channel cannot publish a root.
   The full upload receipt is checked in memory; only uploaded state persists.
   Restart therefore repeats the v2 upload before commit. Tombstones have no blob.
3. Commit sends the exact persisted operation JSON. Matching commit receipts and
   their conflict/head data persist atomically. Interrupted commit responses are
   safely retried with identical ciphertext/operation and fresh proof JWTs.
4. A complete bounded changes page plus every nondeleted **root envelope** must
   pass exact hash/context/AES authentication through native-held keys before
   `stage_changes` atomically advances `received`. Parent/head/tombstone history
   is retained. `RunReport.conflicts` counts conflict commit receipts in that run;
   received branches are independently queryable through durable `heads`.
5. Transport never calls `mark_applied`. Authenticating the root does **not**
   establish typed content/chunk/asset completeness or owner document/user-state
   application. The reader owner must validate the typed graph and atomically
   commit its projection and `applied` cursor via `mark_applied`. Failures roll
   both back. This integration is still missing.
6. Ack retries only the durable `applied` cursor, never `received`. The matching
   authenticated response alone advances `server_acknowledged`. Server revocation,
   unauthorized proof, conflicts, quota, malformed protocol, transient network
   failure and local persistence/authentication failures have sanitized typed
   errors. No failure clears the outbox or acknowledges unprojected data.

## Local synthetic evidence and reproduction

No fixture reads a profile, history, credential file or real issuer. Tests run a
local ephemeral HTTPS bridge with a temporary test CA, the actual production
Worker bundle, real workerd SQLite Durable Object storage and R2 bindings via
Miniflare. Rust generates and retains candidate/device ES256 keys and library
keys. A synthetic RS256 issuer supplies standard nonce-bound ID tokens; actual
Worker challenge/bootstrap verifies them. The synthetic native authority is test
only and intentionally does not claim native OIDC/PKCE/custody acceptance.

From the web worktree, prepare the production bundle without deployment:

```sh
pnpm exec wrangler deploy --dry-run --config wrangler.sync.jsonc --outdir .worker-build/sync
```

From the native worktree, use an isolated Cargo target and already locked packages:

```sh
cargo test -p mdreader-sync --features transport --offline --locked
MDREADER_SYNC_WEB_WORKTREE=/absolute/path/MDReader-Web-reader-next \
  cargo test -p mdreader-sync --features transport transport::tests::actual_worker \
  --lib --offline --locked -- --ignored --test-threads=1
cargo test -p mdreader-service --features sync-transport --test sync_owner --offline --locked
cargo clippy -p mdreader-sync --features transport --all-targets --offline --locked -- -D warnings
```

The receipt regression first failed because weak upload receipt admission advanced
state. It now rejects weak/cross-operation/account/library/device/blob/size/version
and unknown-field receipts without changing pending state. Actual Worker tests
cover lost commit response, byte-identical retry, persisted reopen, independent
received/applied/ack, atomic projection rollback, conflict branches and explicit
tombstone, revoked access, owner unlock during network, scope/generation changes,
per-scope exclusion across transport instances, cancellation/retry, redirects and
body/transfer bounds. The stable checks passed: **22 ordinary crate tests** plus the focused diagnostic redaction regression
(including the subprocess helper; 4 integration cases explicitly ignored there),
**4/4 actual Worker bridge tests**, **2/2 service owner tests**, and focused
transport Clippy with warnings denied. The receipt test observed its expected
behavioral RED before the v2 check was implemented. Tests intentionally ignore actual-Worker cases in the
ordinary suite until the explicit local fixture command is supplied.

## Exact incomplete gates

Production native sync remains unavailable. Missing work includes verified real
issuer/PKCE sign-in and refresh, secure native signing/wrapping/library-key custody,
verified pairing/recovery and rotation, library/account selection, typed encrypted
chunk/asset network fetch and owner integration of memory-only
[incoming preparation](INCOMING.md) with MIME/path/hash/revision authority,
projection into reader documents/pins/revision-scoped positions with local edit
barriers and conflict/deletion policy, user-visible pending/conflict/quota controls,
explicit retention/orphan cleanup/export/account deletion, supported-host native
and mobile multi-device/offline/recovery acceptance and deployment review. The
legacy plaintext raw-key recovery artifact is not used by this transport or its
custody contract. Nothing in this slice changes the content envelope or AAD v1,
activates product sync or establishes cross-host acceptance.

Primary API references checked: [reqwest 0.12 ClientBuilder](https://docs.rs/reqwest/0.12.28/reqwest/struct.ClientBuilder.html),
[jsonwebtoken 9.3](https://docs.rs/jsonwebtoken/9.3.1/jsonwebtoken/), and the checked-in
web Worker protocol/auth implementation. No live issuer or deployment was used.
