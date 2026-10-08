# Native authority foundation: default off, synthetic evidence only

`mdreader-sync/native-authority` is a native-only foundation, disabled by default.
This checkpoint passed focused crate and synthetic integration checks. No product
command, sign-in button, configured real issuer,
production enrollment, deployment or live credential access is supplied. The
previous bounded transport evidence in [TRANSPORT.md](TRANSPORT.md) is distinct
from the authority checks below; neither establishes real issuer/host acceptance.

## Native ownership and interface

The native service explicitly constructs `ReviewedOidcRegistration` from reviewed
issuer, public-client audience, authorization/token/JWKS endpoints and exact HTTPS
sync origin. It cannot deserialize this registration from renderer JSON, discover
an issuer from a token/callback, or use an insecure injected HTTP client. Native
`NativeBrowser` dispatches an authorization URL to the system browser. The native
callback owner must bind/listen on the exact `127.0.0.1` nonzero port and callback
path **before** opening that browser; this crate does not implement that listener
or OS browser adapter. No renderer token, raw key, provider grant or callback RPC
is accepted here.

`NativeAuthorityFactory::begin_login` captures the controller epoch and creates a
single-use five-minute code/S256 PKCE attempt with independent random 256-bit
state, nonce and verifier. Only one attempt can exist in-process. Opening is
one-shot. `verify_login` consumes the attempt on success or failure, validates the
exact callback path, duplicate parameters, state and optional issuer, exchanges
code natively and fetches only the pinned JWKS endpoint. RS256/ES256 signature,
issuer, audience/authorized-party, subject, nonce, expiry and fresh issuance are
verified before computing account SHA256 over compact UTF8 `[issuer,subject]`.
Unsupported critical JOSE extensions and remote key references are rejected.
The resulting `VerifiedLogin`/`VerifiedIdentity` cannot be constructed or serialized
by external callers. Access/refresh tokens are discarded; the verified ID token
exists only in native session memory and signs Worker bearer-bound proofs.

`SessionController` exposes the exact active-scope `RwLock` barrier required by the
borrowed native `TransportOwner`. The owner installs it; it must not invent a
separate binding. Epoch changes invalidate old capabilities even when account,
library and device are equal again. Every remote 401/403 invokes the transport's
`Authority::rejected` hook; using direct `Transport::run` does not skip rejection.
Late failures and dropping an old capability only invalidate the old lease, never
a replacement. Expiry, explicit logout and dropping the active capability clear
its owner binding. Keys and offline records remain intact. No owner/database lock
is held while awaiting OIDC, enrollment, keyring or transport work.

## Synchronous incoming publication fence

A current verified `NativeAuthority` can mint a `ProjectionPermit`. It carries
no credentials or decryption material. Every activation has a fresh UUID token,
including same-scope login and controller restart; a matching scope alone does
not authorize an old operation.

`with_current` holds the existing session binding barrier across short synchronous
owner work. Call `ProjectionGuard::check` after all owner/receipt/cursor SQL,
immediately before COMMIT. Keep network, custody, copies and pixel decoding outside
this closure; do not call logout/reactivation or drop the active authority inside
it. The host must independently fence the concrete selected owner and its operation
lifetime. This API grants no library selection or document mutation by itself.

`SyncStore::mark_applied_guarded` accepts typed application errors and a final
transaction-aware fence. The fence sees provisional owner writes and runs even on
already-applied replay. Failure rolls back application writes and the applied
cursor together. Wall-clock expiry can still make a committed operation's return
uncertain; reconcile durable receipts before retry/acknowledgment. The existing
`mark_applied` API remains available for owners with their own authority boundary.

Focused evidence: two native projection tests passed (same-scope replacement,
controller restart, session barrier and expiry); one real temporary SQLite test
passed (precommit refusal rolls back owner receipt/cursor through reopen; replay
still checks the fence). Strict all-target Clippy with `native-authority` passed.
These checks do not establish native product incoming integration or live login.

## Custody record and crash/restart

`prepare_pending` creates a random opaque `native-sync-v1-<UUIDv4>` reference,
ES256 signing PKCS8 and one 256-bit library key after verified login and exact
local library UUID selection. It performs no OS or network I/O. The native owner
must durably save that opaque reference **before** `persist_pending`; it must retain
the same reference/material after an uncertain OS response. There is no automated
replacement, deletion or orphan-key cleanup API.

Only the dedicated `<app-identifier>.native-sync-credentials-v1` OS vault holds the
strict bounded private version-1 record: opaque reference, registration digest,
account/library/device scope, generation, pending/confirmed enrollment state,
PKCS8 and at most sixteen UUID-addressed 32-byte library keys. Its maximum size is
16 KiB. Restore validates all fields, derives the public JWK/RFC7638 thumbprint
again, rejects mismatched device/account/library/configuration, and never imports
legacy recovery. The record is not a public DTO or renderer serialization format.
`SyncVault` is a trusted native adapter boundary; arbitrary implementations have
native authority, not untrusted plugin access.

`SyncVaultClient` owns one persistent blocking OS worker and one queued operation.
Requests have explicit deadlines of at most ten seconds; factory requests allow
five seconds. A timed-out or cancelled mutation may finish. Reconcile by getting
that same reference through the same worker; do not spawn replacement workers or
regenerate/delete keys. Create is idempotent only for identical material and state.
CAS allows pending generation N to confirmed N+1 only, preserving exact key
material. The OS vault adapter serializes mutation in-process; cross-process app
custody requires the native service/library exclusivity gate. OS keyring behavior,
prompts, platform entitlement and crash acceptance have not yet been exercised.

Restart begins signed out. Fresh verified OIDC login plus the stored opaque
reference and exact local library selection are required. `resume` loads custody,
signs GET `/v1/enrollment`, verifies exact enrolled account/library/device receipt
and bounded P-256 public recovery anchor, rereads custody after network and performs
pending→confirmed CAS before activation. The server public anchor proves existing
enrollment only; it does **not** prove native recovery-key possession or permit
recovery export. A missing/revoked device remains inactive. Factory code never
calls bootstrap/pair/recovery, and normal signing policy refuses those routes.

Buffers use best-effort zeroization and sanitized/redacted diagnostics. JOSE,
reqwest, OS APIs and allocators can make transient private copies. This is not a
hardware-backed/nonexportable-key or comprehensive process-memory-erasure claim.
Desktop keyring backends reuse locked macOS/Windows/Linux support; mobile custody
is explicitly unsupported. No secrets enter SQLite, logs or renderer objects.

## Protocol and acceptance gates

Content envelope/AAD v1 is unchanged. Proofs are standard ES256
`mdreader-proof+jwt` with public JWK, fresh UUID jti/time, exact method and HTTPS
URL including query, SHA256 bearer and SHA256 exact body. Native signing policy
allows bounded changes, encrypted blob GET/PUT, commit and ack only. Uploads bind
canonical operation/library query and exact envelope hash. Transport still stages
only **received** roots: no typed chunks/assets projection, `mark_applied`, or ack
of unapplied content is added by this authority slice.

Production bootstrap, persistent wrapping key, user-verified pairing, recovery
export/activation, rotation and native library selection remain blocked on a
reviewed complete lifecycle. The locked ring public ECDH primitive is ephemeral;
no ad hoc persistent wrapping or discarded recovery secret substitutes for that
review. Test-only enrollment may use synthetic wrapping/recovery keys and a
challenge-bound synthetic issuer token; those constructors/routes are inaccessible
in production authority code.

The current Worker accepts a single configured `OIDC_CLIENT_ID`, also used for
its challenge audience. Native and browser public clients generally require
separate registration identifiers. Exact standard issuer/client compatibility and
a reviewed server acceptance contract remain a gate; this slice does not widen
Worker acceptance. Real issuer/browser/listener dispatch, refresh/relogin UX,
OS-vault runtime/restart, supported-host/mobile and multi-device acceptance,
projection/conflict/quota UI and production deployment remain separate gates.

## Observed focused checks and reproduction

The coordinator granted one heavyweight build slot; all Cargo commands used two
jobs and the existing compatible sync cache. No app/workspace suite ran.

```sh
CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/Users/mlong/Library/Caches/MDReader/sync-transport-target \
  cargo test -p mdreader-sync --features native-authority authority::tests \
  --lib --offline --locked
CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/Users/mlong/Library/Caches/MDReader/sync-transport-target \
  cargo clippy -p mdreader-sync --features native-authority --all-targets \
  --offline --locked -- -D warnings
```

Observed: **8/8 focused authority cases**, **1/1 actual local HTTPS Worker case**
(1.85 s), and **31 affected crate tests passed, 5 explicit integration cases
ignored** in the ordinary suite. Strict affected all-target Clippy with warnings
denied, default-feature-off check, rustfmt and diff checks passed. The first compile
failed because the private keyring module shadowed the dependency; explicit
`::keyring` paths fixed it. This code-only-first slice does not claim an observed
behavioral RED before implementation. No live accounts or OS keyring reads ran.

The ordinary cases cover signature/issuer/audience/nonce/time, PKCE/state one-shot
flow, uncertain vault mutation/reopen, exact scope/configuration, secret redaction,
logout during login, same-scope late 401/replacement, expiry/drop and bounded
signing policy. One explicit synthetic integration case uses the actual local HTTPS
Worker/SQLite/R2 bridge for standard code+PKCE/JWKS, lost bootstrap response,
existing-enrollment reconciliation, native proof transport, restart and revoked
device rejection. Run that case only after ordinary checks are stable and the web
Worker dry-run bundle is current (no deploy):

```sh
MDREADER_SYNC_WEB_WORKTREE=/absolute/path/MDReader-Web-reader-next \
 CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/Users/mlong/Library/Caches/MDReader/sync-transport-target \
 cargo test -p mdreader-sync --features native-authority authority::tests::actual_native_oidc \
 --lib --offline --locked -- --ignored --test-threads=1
```

Dependencies reuse locked ring 0.17, jsonwebtoken 9.3 and keyring 3.6 plus existing
transport/zeroize dependencies. Offline lock resolution changes only workspace
package dependency metadata, preserving registry versions/checksums.

Primary standards: [OIDC ID-token validation](https://openid.net/specs/openid-connect-core-1_0.html#IDTokenValidation),
[native loopback redirects](https://www.rfc-editor.org/rfc/rfc8252#section-7.3),
[Google native OAuth registration](https://developers.google.com/identity/protocols/oauth2/native-app),
[jsonwebtoken 9.3 validation](https://docs.rs/jsonwebtoken/9.3.1/jsonwebtoken/struct.Validation.html).
These standards guide the contract; no real issuer acceptance is claimed.
