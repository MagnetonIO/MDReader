# Native user-verified pairing checkpoint (c)

Implementation is native-only and default-off. Product owner, Tauri commands,
account UI and mobile vault adapters remain separate integration work. This
checkpoint adds no dependency, Worker protocol or content-envelope change.
The native API passed the focused synthetic checks below. Product composition
and live deployment acceptance are separate gates.

## Candidate

1. Complete ordinary verified OIDC/PKCE login for the selected shared library
   UUID. `prepare_candidate(..., CandidateKind::Pair)` creates signing/wrapping
   material with **no library keys**. Persist its opaque reference in the native
   owner and the exact pending material in the OS vault.
2. `prepare_pair_login` obtains a bounded HTTPS challenge of kind `pair` and
   persists the exact candidate request before returning `EnrollmentLogin`.
   `login_enrollment_desktop` performs fresh PKCE with that challenge nonce;
   issuer, ID client audience, nonce, access resource audience and same subject
   remain mandatory. Tokens/PKCE state stay in memory.
3. `request_pair` records uncertainty before dispatching the immutable POST to
   `/v1/pairings`. The server packet must equal the locally reconstructed packet
   including both JWKs/thumbprints, account/library, pair ID, nonce and expiry.
   `pair_request_packet` reopens this public packet after a lost reply; it does
   not claim server acceptance or give the candidate any keys.
4. Transfer `PairRequestPacket::out_of_band_text()` through a separate channel
   whose origin the user actually verifies. No server roster, automatic approval
   or six-digit comparison is treated as a trusted-channel substitute.
5. Receive the reverse `PairApprovalPacket` through that verified channel. Call
   `accept_pair(..., true)` with a current verified login. The sender JWK/thumb
   and grant/JWE hashes are pinned from this packet, never server delivery.
   ES256 grant purpose, exact candidate/scope/nonce/challenge bindings and trusted
   `apu`/`apv` are checked before accepting decrypted library keys.
6. The trusted packet and encrypted delivery are persisted in the OS vault while
   candidate keys remain empty. Only signed `GET /v1/enrollment` confirming exact
   account/library/device and one generation CAS can install the exact keys and
   activate authority. `reconcile_pair(..., true)` repeats this last step after
   restart using the already verified packet/delivery, not a new server sender.

The pair payload transfers library keys only. It does **not** transfer the
independent recovery private anchor, and a paired device cannot export a recovery
file by generating a replacement anchor. Existing bootstrap/recovery holders keep
their recovery capability.

## Approver

`approve_pair(authority, trusted_request, true)` requires a currently active
native authority from the same factory/controller. Packet parsing is strict,
bounded to 8 KiB and rejects duplicate fields/JWK members. Scope, both public
keys/thumbprints, freshness and self-approval are checked.

The existing standard ECDH-ES+A256KW/A256GCM JWE is created once. The HTTPS approval
challenge must bind its hash and the complete request context. The ES256
`mdreader-pair-grant+jwt` binds that challenge and the same context. The exact
challenge/grant/JWE and reverse public packet enter one bounded OS-vault slot
before any approval dispatch. State moves prepared → uncertain before POST and
uncertain → confirmed only after the exact candidate enrollment receipt.

`retry_pair_approval(authority, true)` is explicit and byte-identical; HTTP never
retries automatically. It can reopen an uncertain approval after native restart
and fresh login. `finish_pair_handoff(authority, true)` clears only a confirmed
slot after the native owner has handed the public packet to the user. An
uncertain slot cannot be silently replaced or cleared. HTTP 401/403 on an active
approver invalidates that exact lease; a late failure cannot invalidate a newer
replacement lease.

## Persistence and limits

Custody v2 gains an optional pairing slot; v1 raw bytes remain losslessly retained
and cannot acquire wrapping material implicitly. Existing 16 KiB custody, 4095
byte pair JWE/grant, 16 KiB decrypted payload, 1–16 unique key and one-slot bounds
remain enforced. Oversized material fails before persistence or dispatch.
No keys, recovery secrets, tokens or callback URLs are renderer DTOs/log fields.
Zeroizing buffers cover directly owned plaintext; third-party/transient copies
are not claimed comprehensively wiped.

Expired original pair requests are retained. Generic renewal first probes the
same enrolled identity but cannot infer never-enrolled status from 401/403. It
does not replace a pair request/OOB identity after denial. New expired-pair
selection and abandonment of an unresolved approver slot remain explicit product
gates; local offline reading/custody remain intact.

## Focused evidence, 2026-10-08

- Public browser fixture passes exact pinned sender, signature, hashes/context;
  rejects duplicate packet/JWK members and substituted pin/digest/context.
- Synthetic vault serialization/reopen proves candidate remains keyless;
  substituted keys and stale generation cannot cross the activation CAS.
- Extended the existing single actual local HTTPS OIDC/PKCE + Worker/SQLite/R2 case:
  lose an approval reply, restart/retry identical body, reject unconfirmed/wrong
  OOB packet, lose final enrollment reply with candidate still keyless, then
  restart/reconcile. Worker uses browser JOSE verification of native grants/JWE
  profile; the public fixture covers browser-produced grant/JWE consumption.
- Affected native-authority tests and strict all-target Clippy passed using the
  assigned sole check slot, jobs 2 and the existing warm target.

Executed from the native feature worktree with `CARGO_BUILD_JOBS=2` and
`CARGO_TARGET_DIR=/Users/mlong/Library/Caches/MDReader/benchmark-target`:

```sh
cargo test --locked -p mdreader-sync --features native-authority authority::pairing::tests -- --nocapture
cargo test --locked -p mdreader-sync --features native-authority --lib browser_pair_fixture_requires_exact_out_of_band_pin_and_context -- --nocapture
cargo test --locked -p mdreader-sync --features native-authority --lib authority:: -- --nocapture
cargo test --locked -p mdreader-sync --features native-authority --lib actual_native_oidc_custody_lost_bootstrap_reconnect_transport_and_revocation -- --ignored --nocapture
cargo clippy --locked -p mdreader-sync --features native-authority --all-targets -- -D warnings
cargo test --locked -p mdreader-sync --features native-authority --lib native_pair_and_recovery_interoperate_with_browser_jose -- --ignored --nocapture
```

Initial compile passed (6.02 s); two pairing regressions passed (0.12 s). For the
second command, removing **only** the OOB grant-hash comparison produced expected
RED: substituted `grantHash` was accepted. The exact original source was restored;
the affected authority run then passed 23 tests (0.66 s), with three explicit
integration cases ignored by that ordinary run. Two relevant integration cases
were invoked explicitly: the extended actual HTTPS case passed (5.47 s, 18 real
code/PKCE exchanges, zero rejected verifiers); native-produced pair/recovery JWE
decrypted by browser JOSE passed (0.07 s). The separate full recovery-payload/anchor
JOSE case was already verified at checkpoint (b), and was not repeated because its
implementation did not change. Strict all-target Clippy passed (3.01 s), and
format/diff checks passed. No lock/dependency edits, broad suites, app packaging
or deployment occurred.

The two explicit integrations used
`MDREADER_SYNC_WEB_WORKTREE=/Users/mlong/Documents/Development/.worktrees/MDReader-Web-public-oidc`.
The existing local production Worker bundle SHA-256 was
`a1c261e4c20feab08ef1c36598921b115168b223790227738d8bb9d68ea86d95`;
companion lock SHA-256 was
`002c1fa2109b5a8f7a5517f9a61fb44f2bafe6b5301c510bbe577b3b8ea157ad`.
The public browser fixture remains byte-identical, SHA-256
`73b85ba066e4d927201263bedfb54a86ece492bb8657372c14d5671611e03dc2`.
These pins identify tested local artifacts, not a newly deployed source revision.

No live issuer, deployed pairing, native product account UI, claimed mobile
redirect, mobile keychain or complete cross-device sync acceptance is asserted.
