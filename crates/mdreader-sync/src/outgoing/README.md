# Selected outgoing graph foundation

Tested transport/storage foundation, not product admission or release acceptance. No QueueDocument
intent, RPC/UI, implicit document enumeration, source grant or self-echo cursor
advance is enabled here. Existing envelope/payload/AAD and Worker protocol remain
unchanged. No dependency/version change is required.

`seal_content` builds one bounded root and its immutable encrypted Markdown/asset
chunks. `NativeAuthority::seal_selected_content` uses only native custody material
and rechecks its exact lease before/after crypto. Until a reviewed active-key
rotation policy exists, any library with other than exactly one key fails closed;
no sorted/first-key heuristic is used. Local native UUID/BLAKE3 revision authority
must remain separate from generated wire item/revision UUIDs.

`enqueue_graph_in` borrows the caller's actual SQLite transaction. It never opens
another writer, starts a nested transaction, commits, uploads or advances a
cursor. A future service selected-owner operation must use this same transaction
for its document association and invoke the core current document/assets/source
fence immediately before commit. On any error it must roll back the whole caller
transaction. No file or network work belongs inside this transaction.

The independent additive `store/migrations/001-graphs.sql` namespace persists
root/dependency contexts, exact ciphertext and scoped receipts. Admission reserves
all graph nonces together and bounds global root, dependency, receipt and metadata
charges. Exact operation replay preserves bytes/context; changed root or dependency
sets (including each exact AAD context) are rejected. Partial/future namespace
objects and versions fail without reset; retained orphan dependencies cannot
degrade to the legacy root-only channel. Durable bodies/text are byte-bounded
before allocation, with a separate aggregate ciphertext read bound. Prior
root-only outbox operations retain their supported behavior.

Foreground transport uploads only chunks without confirmed exact v2 receipts.
Each receipt binds account, library, device, operation, blob SHA256 and byte count.
The root's scoped upload receipt and uploaded state commit together, only after
all dependencies are confirmed. A weak legacy receipt cannot admit a graph root.
Commit-receipt admission rechecks this dependency/root proof as well.

When the run's byte budget cannot admit the next upload plus bounded receipt,
progress remains durable and the report stays pending. A later explicit foreground
run resumes exact bytes; no automatic retry/background owner is added. A lost
response can reupload only the unconfirmed immutable body. Root upload/commit is
never a typed incoming apply; received/applied/server-ack cursors stay separate.

Focused checkpoint (2026-10-08), using the existing warm Cargo target,
`CARGO_BUILD_JOBS=2`, locked/offline resolution and synthetic local SQLite/R2/TLS:

```sh
cargo test --locked --offline -p mdreader-sync --features native-authority --lib store::graph_tests:: -- --nocapture
cargo test --locked --offline -p mdreader-sync --features native-authority --lib transport::tests::actual_worker_immutable_retry_scoped_receipts_staging_and_applied_only_ack -- --exact --ignored --nocapture
cargo clippy --locked --offline -p mdreader-sync --features native-authority --all-targets -- -D warnings
```

The SQLite run first produced **2 PASS / 1 RED** (0.57s): the future-schema case
revealed that an existing revision's reuse fast path ran before schema admission.
Validating the namespace before replay restored **3 PASS** (0.55s; 2.09s compile).
The same three cases cover caller transaction rollback/reopen, nonce reuse,
exact context/ciphertext replay, strict immutable dependency/root receipts,
aggregate quota, orphan detection and future-schema preservation.

The existing actual HTTPS Worker fixture passed **1 case / 2.26s**, including
its prior scenarios plus partial graph progress, reopen, connection loss after
Worker chunk storage, byte-identical retry and root publication only after all
chunk receipts. It decrypts the downloaded graph and confirms that outgoing
publication/preparation never advances the applied cursor. No additional issuer
fixture or broad matrix was run. `MDREADER_SYNC_WEB_WORKTREE` referenced the
reviewed companion integration checkout (observed HEAD
`8be2b785c1494703505cd6cab2c9541cb7ed5f06`); exact local Worker bundle SHA256:
`fb22baded8f9b9db8aaa44962bb021962c4f2dd9333780250825ce7950c98a22`.
This is local synthetic Worker acceptance, not a deployment or live login.

Strict all-target Clippy passed **2.22s** after fixing one needless borrow in the
extracted root-admission helper. Formatting and diff checks passed. No dependency,
package version or lockfile changed. The separate core read-only publication fence
is deliberately excluded from this checkpoint.

Service selection remains a later checkpoint: exact native document and asset
snapshot, selected scope/epoch fence, durable association+graph transaction,
receipt-proven self-echo to the same native document (without duplicate import),
dirty baseline CAS, conflict/tombstone fencing and pin/position preservation.
Product recovery/mobile/live issuer acceptance and bounded orphan cleanup remain
separate gates. Do not expose selection until those associations are verified.

## Linear dependency receipt checkpoint (2026-10-08)

Per-chunk receipt admission previously loaded/hashed every sibling body for each
receipt, multiplying graph work by up to 72 chunks under owner access. Admission
now validates only the exact addressed row (at most 512KiB), its scoped graph
membership, ordinal, context, envelope key, ciphertext SHA256/byte count and prior
immutable receipt. Root publication and commit still verify the whole graph and
all receipts. The commit gate no longer repeats the same complete graph proof.
No receipt status string substitutes for those identity/integrity checks.

The existing receipt regression now corrupts a sibling context: admitting the
valid target receipt succeeds, while full root publication fails until that
sibling is restored. Restoring only the old full-graph-per-receipt call produced
expected RED in 0.15s (compile 2.29s). Exact final source restored, the same three
SQLite cases passed in 0.52s (compile 1.65s); the existing actual HTTPS Worker
fixture passed in 2.12s; strict sync/native-authority all-target Clippy passed in
1.92s. Commands, synthetic fixture and artifact are the same as above. No timing
benchmark or release-performance claim is inferred from these behavioral checks.

Service association and new borrowed graph-proof seams were excluded from this
compiled checkpoint. They remain separate source work with no QueueDocument RPC
or UI enabled. Full graph verification still has a bounded aggregate ciphertext
read/hash cost at publication; it is not file/image/model or network work.
