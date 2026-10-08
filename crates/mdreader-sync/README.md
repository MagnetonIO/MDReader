# MDReader encrypted sync client primitives

This library supplies the native cryptographic wire format and durable local sync state. The default feature set performs no networking, login, device enrollment, transcript reads, database opening, keychain access or background work. Optional `transport` supplies a bounded native-only HTTP foundation with injected authority; see [TRANSPORT.md](TRANSPORT.md). The reader service supplies its exclusively owned `&mut rusqlite::Connection` and remains the only SQLite writer. This is a foundation for Task 4, not completed product sync or acceptance evidence.

## Wire compatibility and keys

The copied synthetic fixture at `fixtures/a256gcm-v1.json` is byte-identical to the browser fixture in `MDReader-Web/lib/reader/fixtures/a256gcm-v1.json`. Tests authenticate its plaintext and reproduce its exact envelope and SHA-256 blob ID using the fixed test-only nonce. Production callers cannot supply a nonce.

`LibraryKey::generate` obtains a fresh 256-bit key and UUIDv4 identifier from OS entropy. `LibraryKey` is neither cloneable nor serializable, redacts its Debug output, and zeroizes its bytes when dropped. The AES implementation's `zeroize` feature also wipes expanded key state. Decrypted plaintext is returned in `Zeroizing<Vec<u8>>`; callers remain responsible for copies they create.

`EncryptionContext` requires canonical lowercase UUIDv4 identifiers. AAD is exactly the compact UTF-8 JSON array `["mdreader",1,libraryId,itemId,revisionId,keyId]`. The serialized envelope property order is `v,alg,keyId,nonce,ciphertext`, with `v:1`, `alg:"A256GCM"`, a 12-byte unpadded base64url nonce, and unpadded base64url ciphertext followed by its 16-byte authentication tag. Blob identity is lowercase hexadecimal SHA-256 of the exact transmitted envelope UTF-8. Unknown/duplicate envelope fields, unsupported versions/algorithms, noncanonical encodings, wrong contexts and failed authentication are rejected.

`encrypt` creates a fresh random nonce. The durable service path is `SyncStore::seal_enqueue`, which commits a reservation keyed by the actual key fingerprint and nonce before encryption. Retries of the same operation return the existing encrypted bytes after authenticating the requested plaintext; failed enqueue attempts leave their nonce reserved. The nonce ledger must travel with the library database and must not be cleared independently of key rotation. Restoring a database snapshot cannot detect reservations made after that snapshot; OS randomness still provides nonce collision resistance, and restoring/duplicating a writer should rotate the key. Independent devices also rely on independently generated random nonces. This is not a globally coordinated nonce allocator.

Envelope size is bounded at 2 MiB to match the server. Plaintext is bounded at 1 MiB to leave base64/metadata headroom. A larger native document is rejected before enqueue; product code needs an explicit larger-document policy rather than silently acknowledging it. Limits also bound outbox rows/bytes, remote revisions/bytes, nonce reservations, page sizes, revisions per item and heads per item. Quotas count all local scopes, including acknowledged records; nothing is silently garbage-collected.

`PreparedRevision::from_wire(key, context, expected_blob, bytes)` checks size, exact blob digest and AES authentication before a download can enter durable remote state. `PreparedRevision` retains only context and ciphertext. Opaque UUIDs, hashes, parent links, sequence numbers and device identifiers are the only clear metadata. Titles, paths, reading state and document text belong inside the encrypted application payload. Provider credentials, raw histories and private tool logs must never be submitted.

`export_recovery` and `import_recovery` provide a bounded, versioned, library-bound **raw-key offline recovery artifact**. The export is sensitive plaintext key material with a corruption checksum. It is not password encryption, authenticated provenance, account recovery, cloud custody or device pairing. Keep it user-controlled and out of SQLite, logs, crash reports and network requests. An explicit export UI, secure key custody and verified pairing/enrollment remain product work; this crate does not invent a working login or pairing flow.

## Owner connection integration

Call `migrate(&mut owner_connection)` from the service owner before attaching. Migration uses only `mdreader_sync_*` tables and an independent version guard; it preserves the reader's `PRAGMA user_version`. Orphaned/incomplete and future schemas fail closed. The owner configures durability: `SyncStore::attach` requires `synchronous=FULL` or `EXTRA` and an on-disk journal mode (`WAL`, `DELETE`, `TRUNCATE` or `PERSIST`). It does not change owner pragmas. Its mutable borrow prevents concurrent use of that connection during a store's lifetime.

Create `SyncScope::new(account_digest, library_id, device_id)` only from verified transport identity and the selected local library. The account digest is SHA-256 of compact UTF-8 JSON `[verifiedIssuer,subject]`; device ID is the registered P-256 JWK thumbprint encoded as 43-character unpadded base64url. Every response transition checks the supplied scope against the attached scope. Switching account, library or device cannot see or acknowledge another scope's entries. Operation and revision IDs are additionally unique per account, matching server replay rules.

These APIs validate response bindings and state transitions; **they do not authenticate HTTP responses**. The service must supply responses obtained through its authenticated transport and bind `response_scope` to the request's verified identity. A caller-created matching JSON object is not proof of server acknowledgment.

Upload sequence:

1. For a selected local revision, call `seal_enqueue(operation_id, parents, key, context, plaintext)`. Queue acknowledgment follows its durable SQLite commit. Use `enqueue_tombstone` for deletion; tombstones have no blob. `enqueue_prepared` supports an already authenticated/encrypted body.
2. Read `pending(1..=100)`. PUT the exact stored envelope to `/v1/blobs/{blob}`. Keep the entry pending on transport interruption. `mark_uploaded` accepts only 200/201 with the matching blob and exact envelope byte count. A tombstone starts in uploaded state because there is no blob upload.
3. POST `entry.operation.wire_bytes()` to `/v1/commit`. `commit_receipt` accepts only a matching operation/revision, valid sequence and consistent retained heads after upload. Only then does the row become acknowledged. Same receipt replay succeeds; changed receipt, reused operation payload and cross-scope response fail. Acknowledgment retains ciphertext and the receipt for recovery/export.

Download/application sequence:

1. Fetch changes after `cursors().received`. Fetch every new nondeleted blob and create a `PreparedRevision::from_wire` with that change's exact context and trusted library key.
2. `stage_changes(response_scope, status, after, page_bytes, prepared)` atomically stores ciphertext, opaque metadata and branch heads, then advances only `received`. Page gaps, tampered/missing bodies and invalid parent relations leave the cursor unchanged. Tombstones and concurrent heads remain available through `heads(item_id)`; `staged(after, limit)` also includes consumed historical revisions. A late offline branch does not erase a deletion. Only a revision naming all current heads resolves that conflict.
3. Apply staged revisions with `mark_applied(through, |transaction| { ... })`. The owner's document/reading-state SQL and the applied cursor commit in the **same transaction**. Failure or process death rolls both back. `through` must cover only revisions whose application has actually completed. For filesystem projections, the owner must durably publish replay-safe files before this transaction's commit and retry safely after interruption; SQLite cannot make arbitrary filesystem writes atomic.
4. Only after that commit, POST `/v1/ack` with `applied`. Pass the authenticated response to `confirm_cursor_ack`. It cannot acknowledge beyond locally applied state. Network failure leaves applied data intact and the acknowledgment retryable.

Remote conflict/tombstone state and outbox receipts are retained without automatic resolution, overwrite or collection. Quota exhaustion returns a visible error and leaves unacknowledged work retryable. Explicit user-directed conflict resolution, retention/GC rules and safe key rotation remain service/product policy.

## Evidence and remaining acceptance

The tests use synthetic fixtures and real temporary SQLite files, including reopening unacknowledged outboxes and killing a subprocess inside an uncommitted owner application transaction. They cover browser/native known answers, tamper/context rejection, nonce reservation across reopen, malformed receipts/pages, scoped transitions, conflicts, late offline edits after deletion, explicit merges, schema guards, quotas and cursor boundaries. No deployed account, private key, provider credential or user history is used.

Historical foundation checks before the codec/transport additions: `cargo test -p mdreader-sync` passed 18 test entries (including the subprocess helper); `cargo clippy -p mdreader-sync --all-targets -- -D warnings` passes; fixture comparison and `git diff --check` pass. Windows GNU `cargo check` was attempted and blocked in bundled SQLite's build script because `x86_64-w64-mingw32-gcc` is unavailable on this host; it provides no Windows compile/runtime evidence.

Task 4 remains incomplete until the service applies the selected document/reading-state schema and conflict UI, provides secure local key custody, implements verified account/device authorization and pairing, connects the optional bounded transport to typed owner projection and production custody, and passes real multi-device recovery/offline/deletion/account-switch tests on supported native/mobile hosts. This crate has no live cloud or cross-host runtime acceptance. The separate capture crate still explicitly rejects Windows capture pending its durable publication implementation.

Primary dependency references checked for this implementation: [RustCrypto AES-GCM](https://docs.rs/aes-gcm/latest/aes_gcm/), [zeroize](https://docs.rs/zeroize/latest/zeroize/), [getrandom](https://docs.rs/getrandom/latest/getrandom/), [base64](https://docs.rs/base64/latest/base64/) and [SHA-2](https://docs.rs/sha2/latest/sha2/). The server protocol authority is the checked-in web `workers/sync/README.md` and worker source; no public enrollment endpoint is assumed.

## Selected encrypted payload codecs v1

`codec` now defines strict content/chunk/user-state payload v1 shapes matching
web `lib/sync/codecs.ts`. The existing AAD/envelope is unchanged. The copied public
synthetic `fixtures/payload-v1.json` contains five independently authenticated
vectors (Markdown chunk, PNG chunk, content manifest, block/pixel anchor and line
anchor), file SHA256
`0c2b1efe7189caca6823a475edb0ba42a60ef126400576522cabf24b79bb4055`.
`payload_codec` decrypts and re-encodes the exact fixture bytes and rejects unknown
ratio/unsafe path/unbounded descriptor shapes. Strict tagged anchors retain line
versus block/pixel semantics; pins survive document revisions while positions
remain revision-scoped. No approximate conversion is provided.

The codec validates shape/UUID/path/digest/count/size bounds and canonical chunk
base64url. Memory-only [incoming preparation](INCOMING.md) now authenticates
resource graphs, reconstructs exact originals, checks raw-byte hashes and admits
bounded PNG/JPEG structure. It performs no owner mutation or cursor advancement
and is not a full image decoder. Native dependency fetching, portable owner
projection and user-state conflict/tombstone policy must still be integrated
before native transfer is accepted. Browser
implementation and exact remaining gates are documented in web
`lib/sync/PAYLOAD-PROTOCOL.md`. The codec slice adds no auth/provider/keychain behavior; the optional transport
foundation is separately described in [TRANSPORT.md](TRANSPORT.md). Current focused transport/owner checks and exact remaining gates are recorded in
[TRANSPORT.md](TRANSPORT.md); no cross-device native acceptance is claimed.

## Native custody and standard wrapping checkpoint

Optional `native-authority` now includes persistent custody v2 and strict browser-compatible pair/recovery JWE primitives. [KEY-LIFECYCLE.md](KEY-LIFECYCLE.md) records the wire profiles, v1 preservation, executed synthetic conformance checks and remaining enrollment/pairing/recovery gates. This does not enable a native product key lifecycle or reuse the raw-key legacy recovery artifact.

Checkpoint (b) adds nonce-bound native enrollment, durable exact request/uncertainty state, explicit encrypted recovery export/import and anchor-verified activation. [NATIVE-ENROLLMENT.md](NATIVE-ENROLLMENT.md) records the public native interfaces, actual local HTTPS/browser conformance evidence and product/mobile/pairing gates. No production composition or deployment is enabled.
