# Native key lifecycle checkpoint (a)

This checkpoint implements native-only persistent key material and standard JWE
conformance. It does **not** implement product bootstrap, pair approval, recovery
activation, export UI or a renderer RPC. `native-authority` remains default-off.
No live account, OS credential record, deployed Worker or installed profile was
changed by these checks.

## Custody boundary and preservation

The existing opaque `native-sync-v1-<uuid>` reference and keyring service name
remain unchanged. Only the native vault adapter handles private record bytes;
the service may retain the opaque reference. New records use strict wire v2 and
retain independent ES256 signing, persistent P-256 wrapping, and (for bootstrap)
ES256 recovery-anchor private keys. Library keys remain client-held. The record
binds reviewed registration digest, verified account, selected library, device
thumbprint and generation. Its serialized size is at most 16 KiB; at most sixteen
library keys are admitted.

Bootstrap-pending records hold real library keys and a recovery anchor. Pair and
recover candidates instead have a distinct typed pending state, zero library
keys and no recovery-anchor key. They cannot resume transport or be confirmed by
the bootstrap transition. There is no placeholder key or implicit acquisition of
library authority. Creation is idempotent only for the same exact material;
compare-and-exchange requires the expected generation, identical private keys
and the permitted pending-bootstrap to enrolled transition. Uncertain writes
retain the same reference/material for reconciliation.

V1 decoding uses its original strict schema and retains original raw bytes,
including member order and whitespace. Unchanged re-encoding is byte-identical.
V1 pending records fail closed for resume; they are neither replaced nor upgraded
by generating missing wrapping/recovery keys. Already-confirmed v1 records retain
their existing signing behavior, without claiming possession of the server's
recovery anchor. Explicit v1 migration/re-enrollment remains future work. V2
resume checks a persisted recovery private key against the server's public
anchor; it does not create enrollment.

Private wrapping/recovery-secret/plaintext wrappers have no public Debug or
Serialize implementation. Raw secret exports and derivation buffers use
`Zeroizing`; AES-GCM/AES-KW expanded states use their zeroize features. Private
wire strings are wiped on drop. These are best-effort in-process protections,
not a guarantee against allocator/parser/compiler copies, swap or crash dumps.
OS vault availability, prompts and host-specific secure-custody acceptance are
separate from the in-memory vault lifecycle tests below.

## Fixed browser-compatible profiles

The content envelope and its AAD v1 are unchanged. Pairing uses RFC 7516 compact
JWE with `alg=ECDH-ES+A256KW`, `enc=A256GCM`,
`typ=mdreader-pair+jwe`, an ephemeral public P-256 `epk`, and exact `apu`/`apv`.
RFC 7518 Concat KDF uses SHA-256, AlgorithmID `ECDH-ES+A256KW`, decoded party
information and a 256-bit key-data length; RFC 3394 wraps the fresh 256-bit CEK.
Recovery uses `alg=dir`, `enc=A256GCM`, `typ=mdreader-recovery+jwe` and an empty
encrypted-key segment with an independently random 256-bit recovery secret.
That secret is not a password and is unrelated to legacy raw-key recovery.

`PairContext::new` requires the account/library/request/nonce/candidate
fingerprints plus the trusted sender fingerprint from the caller's verified
out-of-band context. It derives the exact existing browser party information:

```text
apu = base64url(UTF8(JSON(["mdreader-pair",1,account,libraryId,senderThumb])))
apv = base64url(UTF8(JSON([pairId,signingThumb,wrappingThumb,nonce])))
```

Decryption compares these exact expected values; the protected header cannot
assert its own trust. This primitive does not perform the user verification or
enforce sender provenance on behalf of its caller.

Inputs are bounded before parsing/base64 decoding. Compact shape, canonical
base64url, IV/tag/wrapped-key sizes, public point/curve and exact profile fields
are checked. Strict structs reject unknown or duplicate header/JWK members,
including `crit`, `zip`, remote-key URLs and private JWK fields. Pair JWE is at
most 4095 bytes to match the current Worker bound; recovery JWE at most 32768
bytes; plaintext at most 16 KiB. A key bundle that exceeds the pair bound returns
a typed bounds error, without truncation or partial activation.

Exact optional direct pins are `p256=0.14.0` (`ecdh,std`, defaults off) and
`aes-kw=0.3.1` (`zeroize`). Existing `sha2=0.11` gains `zeroize`. They use the
RustCrypto backend without OpenSSL. Lock resolution adds sixteen transitive
package entries; all preexisting package versions and checksums are unchanged.
Feature dependency edges and existing `signature`/`spki` version disambiguation
also change. Host compilation here does not establish mobile target acceptance.

## Executed focused evidence (2026-10-08)

All Cargo commands used `--locked`, `CARGO_BUILD_JOBS=2` and the existing compatible
cache. Initial compilation found only a custody wire-module visibility error;
the narrow re-export visibility correction preceded these passing checks.

```sh
cargo test --locked -p mdreader-sync --features native-authority authority::wrapping::tests -- --nocapture
# 2 passed; 1 explicitly scheduled interop test ignored in this filter
cargo test --locked -p mdreader-sync --features native-authority authority::tests -- --nocapture
# 11 passed; 1 separate actual HTTPS issuer/Worker integration ignored
MDREADER_SYNC_WEB_WORKTREE=/ABS/MDReader-Web \
  cargo test --locked -p mdreader-sync --features native-authority \
  native_pair_and_recovery_interoperate_with_browser_jose -- --ignored --nocapture
# 1 passed, 0.07s; explicitly executed, not waived
cargo clippy --locked -p mdreader-sync --features native-authority --all-targets -- -D warnings
# passed
```

The companion worktree must have its frozen browser dependencies installed; the
Node subprocess resolves that tree's locked `jose`. It consumes only synthetic
test input. Browser JOSE decrypts native-produced pair and recovery JWEs, checks
the exact party context/plaintext, and produces a recovery JWE that Rust decrypts.
The ordinary wrapping test also decrypts the existing browser-produced pair JWE.
The copied synthetic `fixtures/pairing-v1.json` is byte-identical to the browser
fixture, SHA-256
`73b85ba066e4d927201263bedfb54a86ece492bb8657372c14d5671611e03dc2`.
Other focused assertions cover duplicate/substituted headers, invalid points,
wrong keys/context, tamper, lossless v1 bytes, empty candidate authority,
persistent v2 anchors across reopen and failed/replayed vault CAS.

## Remaining gates

Checkpoint (b) must implement nonce-bound verified bootstrap and recovery
capabilities, durable request/ciphertext intent before network, login/session
fences, exact response revalidation and loss-safe crash/retry reconciliation.
Checkpoint (c) must implement the actual user-verified out-of-band pairing step,
trusted sender pinning, full candidate-key binding and activation only after
verified transfer. Neither follows from a successful JWE roundtrip.

Explicit recovery export/activation confirmation, native product composition,
v1 migration, key rotation/revocation/account deletion, real registered issuer
acceptance, host OS vault restart behavior and desktop/mobile multi-device tests
remain release gates. The synthetic actual-HTTPS authority test was updated to
use persisted v2 public anchors but was not rerun in this checkpoint. There was
no deployment or product activation.

Primary references: [RFC 7518 Concat KDF](https://www.rfc-editor.org/rfc/rfc7518.html#section-4.6.2),
[p256 0.14.0 ECDH](https://docs.rs/p256/0.14.0/p256/ecdh/),
[aes-kw 0.3.1](https://docs.rs/aes-kw/0.3.1/aes_kw/).
