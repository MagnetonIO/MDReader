# Native enrollment and encrypted recovery checkpoint (b)

Optional, default-off native contracts now perform verified first-device bootstrap
and recovery enrollment against the unchanged auth-v2 Worker protocol. The local
synthetic integration uses the actual Worker, SQLite Durable Object and R2, HTTPS
OIDC code/PKCE/JWKS verification, and the real native loopback callback listener.
This is not native product UI/RPC, a mobile secure-vault implementation, live
Auth0 acceptance or a deployment.

## Native owner flow

An initial `login_desktop` establishes the verified account from signed ID and API
access tokens. `prepare_pending` creates independent v2 keys for the selected
library; the native owner saves its opaque reference and calls `persist_pending`.
No unverified JWT/account header determines this account, and no renderer handles
a token or private key.

`prepare_bootstrap(&verified_login, reference, library)` obtains an enrollment
challenge from the configured exact HTTPS Worker endpoint. It checks the v2
profile, resource audience, kind, library, signing/wrapping/recovery fingerprints,
nonce, UUID and short lifetime before persisting the exact candidate POST bytes.
The HS256 token is opaque to this client: HTTPS authenticates its server origin;
the Worker verifies its own signature. The native client does not possess the
server enrollment secret or claim to verify that signature.

The returned native-only `EnrollmentLogin` pins reference, registration, account,
generation, body digest, nonce, expiry and controller epoch.
`login_enrollment_desktop` binds the exact registered loopback address before
opening the system browser and runs a fresh PKCE login with that nonce. The
verified ID token must match it, while the API access token retains the shared
resource audience/client/subject/scope checks. A different account, stale
generation, logout or session replacement cannot submit the enrollment request.
The token pair/state/verifier are never persisted in custody.

`bootstrap(verified_nonce_login, reference, library)` persists uncertainty in the
vault **before** HTTP dispatch. The request body is unchanged; only request proof
freshness changes across attempts. It validates the POST receipt, then reconciles
the enrolled device and held recovery anchor through signed GET before the
pending-to-confirmed CAS and native activation. A lost reply retains the same
keys/reference/request. Fresh ordinary verified login plus `resume` can reconcile
success even after the challenge expires. No new key is generated for that retry.

## Explicit recovery and activation

`NativeAuthority::export_recovery(true)` requires an active native capability and
explicit local confirmation. It returns an encrypted file wrapper and a separate
random 256-bit secret; it performs no file/server write. The owner must present a
local export destination and keep that secret separate from the encrypted file.
False confirmation fails closed. This does not use the legacy raw-key artifact.

The exact existing browser recovery file is
`{v:1,kind:"mdreader-recovery",jwe}` with `dir/A256GCM` compact JWE. Its plaintext is
the browser `mdreader-library-keys` v1 account/library/key bundle plus
`recoveryPrivateJwk`. Private JWK fields are strict, including duplicate rejection.
P-256 scalar-to-PKCS#8 conversion preserves the same point and is checked by the
ring ES256 signer/RFC7638 thumbprint; it never substitutes a recovery anchor.

A new `prepare_candidate(..., CandidateKind::Recover)` has no library keys.
`prepare_recovery` requires verified selected account/library, bounds the selected
file/secret before parsing, decrypts in native memory, and validates all keys and
the original recovery anchor. Its `PendingRecovery` is nonserializable, has no
Debug/key getter, expires after five minutes, and is unusable after logout.

`prepare_recovery_enrollment(..., true)` requires explicit activation confirmation.
It gets a recover challenge and signs the exact existing ES256 recovery-grant JWT
with the imported recovery key. It persists the exact request and decrypted
pending bundle **only inside the OS-vault record** before issuing a nonce-bound
login. The recovery secret is not persisted. The candidate's active key map stays
empty. Reopening `recovery_enrollment_login` reuses this exact request/material.

`recover(..., true)` durably marks dispatch uncertain, sends the signed candidate
request, validates its receipt and checks the server's public recovery anchor.
`reconcile_recovery(..., true)` performs the latter check after a lost reply or
restart using fresh ordinary login. Only the exact source candidate, generation,
scope, signing/wrapping keys, pending bundle, recovery anchor and receipt may CAS
into confirmed keys. Changed keys or replayed CAS fail. Transport cannot use the
staged key material. False confirmation never activates or exports.

## Expiry, bounds and failures

`renew_enrollment(..., true)` is an explicit retry after expiry. It probes the same
enrolled account/device first. Success reconciles; transport/validation failure
stops. A 401/403 is ambiguous, including a revoked device, and establishes no
new-account eligibility. It permits only a fresh **same-material** challenge
attempt. Worker sticky `ever_enrolled` and device-known/revocation checks remain
authoritative and can refuse the subsequent POST. The native record stays pending
on refusal; there is no local regrant or automatic key/anchor replacement.

The prior request's digest, challenge ID, nonce, expiry and sent/uncertain state
are retained on renewal, with a maximum eight prior attempts. Current exact body
and its confirmed scope/anchor receipt remain durable. Overall custody remains
bounded at 16 KiB; large bundles/history fail without partial activation or silent
discard. Recovery JWE and plaintext bounds remain those in
[KEY-LIFECYCLE.md](KEY-LIFECYCLE.md). V1 byte-preserving/no-implicit-upgrade behavior
is unchanged. No database transaction or owner mutex is held across HTTP or vault
awaits. Epoch/scope/generation are checked after external work.

OS-vault records contain private material; renderer state, SQLite, logs, URLs and
telemetry must contain none. JS/library/parser/compiler copies cannot be promised
zeroized; the native wrappers and owned secret buffers use best-effort zeroizing
cleanup. Tests here use only synthetic keys and an in-memory encoded vault; they
do not establish OS vault permission/prompt/restart behavior on every platform.

## Executed evidence (2026-10-08)

Warm compatible Cargo cache, `CARGO_BUILD_JOBS=2`, no packaging/full workspace run:

```sh
cargo update --offline -p mdreader-sync
# 0 packages: only existing ecdsa/signature digest dependency edges for p256 pkcs8
cargo test --locked -p mdreader-sync --features native-authority lifecycle -- --nocapture
# 2 passed, 0.34s; explicit browser test scheduled separately
cargo test --locked -p mdreader-sync --features native-authority authority:: -- --nocapture
# 21 passed, 0.69s; all three integration gates executed explicitly below
MDREADER_SYNC_WEB_WORKTREE=/ABS/MDReader-Web cargo test --locked -p mdreader-sync \
  --features native-authority interoperate_with_browser_jose -- --ignored --nocapture
# Existing pair/recovery JWE gate passed; new recovery payload gate initially failed
MDREADER_SYNC_WEB_WORKTREE=/ABS/MDReader-Web cargo test --locked -p mdreader-sync \
  --features native-authority native_recovery_payload_and_anchor_interoperate_with_browser_jose -- --ignored --nocapture
# New gate passed 0.24s after explicit extractable=true in synthetic re-export harness
MDREADER_SYNC_WEB_WORKTREE=/ABS/MDReader-Web cargo test --locked -p mdreader-sync \
  --features native-authority actual_native_oidc_custody_lost_bootstrap_reconnect_transport_and_revocation -- --ignored --nocapture
# Passed 3.30s: actual HTTPS Worker/SQLite/R2 + 11 code/PKCE exchanges
cargo clippy --locked -p mdreader-sync --features native-authority --all-targets -- -D warnings
# Passed 2.68s after boxed enum payload and test-module placement corrections
```

The first compile also corrected a test-harness URL/string type mismatch. These
failures are recorded as compilation/harness failures, not a claimed behavioral
RED-to-GREEN experiment. Clippy's final boxing correction changes result storage,
not crypto/network behavior. No broad matrix was repeated.

The private-JWK regression proves browser fixture → P-256 PKCS#8 → ring signer
preserves the exact public thumbprint, then exports the same JWK; mismatch and
duplicate-member inputs fail. Locked browser `jose=6.1.3` decrypts native recovery
payloads, verifies a native recovery-anchor signature, re-exports that same key
and returns a browser-produced recovery file that native imports with identical
keys/anchor. Existing pair JWE interoperability also remains passing.

Synthetic HTTPS evidence includes lost bootstrap/recovery replies, restart before
recovery activation, encrypted local export/import, exact pending material,
scope/anchor/CAS checks, transport reconnect, revoked-device rejection and explicit
ambiguous-denial renewal that fails to regrant while preserving history/keys.
Worker bundle SHA-256:
`a1c261e4c20feab08ef1c36598921b115168b223790227738d8bb9d68ea86d95`.
Companion pnpm lock SHA-256:
`002c1fa2109b5a8f7a5517f9a61fb44f2bafe6b5301c510bbe577b3b8ea157ad`.
The bundle is a local checked artifact, not evidence of a new cloud deployment.

## Remaining release gates

Checkpoint (c) user-verified out-of-band pairing is described separately in
[NATIVE-PAIRING.md](NATIVE-PAIRING.md); the evidence above covers (b). Native
product composition/RPC, selected-library UI/consent, real registered issuer
login, secure OS-vault acceptance, mobile system-auth callbacks/vault adapters,
explicit export UI, v1 migration, rotation/deletion and multi-host device testing
remain. Product code must never infer user consent from boolean values supplied
by an untrusted renderer. No production constructor, provider grant or insecure
legacy recovery was introduced to bypass these gates.
