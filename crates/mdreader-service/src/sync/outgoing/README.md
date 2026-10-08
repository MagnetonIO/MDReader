# Selected native document association and self-echo

This checkpoint has focused real SQLite/storage evidence. It introduces a native
blocking owner seam, not a product QueueDocument command, account intent or Settings action.
There are no new dependencies, wire formats, background owners or deployments.

## Admission

`IncomingCoordinator::queue_selected_document` accepts one explicitly selected
native device-document UUID and one stable operation UUID. It reuses the actual
coordinator's ReaderStore, selected ProjectionPermit, AssetRuntime and operation
lease. An actual NativeAuthority must match the selected account/library/device
and current activation. There is no renderer token/key input or authority bypass.

The current native UUID and raw-byte BLAKE3 revision remain distinct from the wire
item/revision UUIDs. A known content head reuses its wire item with that exact head
as parent. Unknown historic baselines, observed tombstones, conflicting mappings
and an outstanding queued operation for the document are rejected. A new explicit
selection creates a fresh wire item only when no native association exists.

Core current/archive preparation captures selected metadata under short owner
access. Exact Markdown, selected tracked PNG/JPEG assets, hashing and encryption
run outside owner/session locks. Missing, partial, retired or unsupported assets
are rejected; there is no arbitrary path lookup. A truly absent asset namespace
may be initialized only through the existing explicitly configured asset owner;
malformed, partial or future namespaces do not fall back to initialization.
Only a single active library key is supported; multiple keys fail closed.

One borrowed SQLite transaction admits the opaque native/wire association and the
entire sealed root/dependency graph, including all nonce reservations. Core's
read-only current fence runs before admission and after all association/graph SQL;
the live session and selection are retained through commit. Errors roll back the
whole unit. Repeating an admitted operation returns its existing exact graph and
identities without resealing. Changing the selected document or pinned activation
cannot silently authorize a prior intent.

The lazy `native_outgoing_*` namespace has exact versioned layouts, a 4,096-intent
history bound and 4 KiB per opaque intent. It stores no Markdown, asset bytes,
titles or private paths. Core's existing document and asset tables remain the
only native content authority. Applied history is retained; there is no automatic
pruning, regrant or old-client downgrade repair.

## Receipt-proven self-echo

Incoming content checks a selected outgoing association before the normal new
import path. A local outbox revision with no association becomes pending rather
than creating a duplicate native document. A self-echo requires the same scope,
activation/selection, immutable operation, own device, revision, item, parents,
blob and sequence; a nonconflicting commit receipt with exactly that head; and all
strict scoped root/dependency upload receipts. An acknowledged status alone is
not proof. The borrowed sync proof reads do not initialize schemas or write data.

The typed decrypted body must equal the selected snapshot. A changed source,
retirement, state generation (including pin ABA), raw revision, asset metadata or
asset generation prevents publication. The final borrowed transaction records
the incoming receipt/head, marks the association applied and advances the applied
cursor together. It preserves the original native document/source/asset UUIDs,
pins and exact line; it writes no replacement files. Server acknowledgement
remains separate and follows applied state through the existing foreground pump.

Current/archive reads verify asset bytes outside the owner lock. Admission and
self-echo both run core's `revalidate_current_archive` before provisional writes
and at final commit. It binds the actual archive/current owner, exact state and
asset metadata/generation, and retains opened Markdown/asset file evidence through
commit. Missing, partial or nonready asset snapshots are rejected. The readonly
fence compares retained and current exact-path file identity/change stamps; it
does not hash bodies, decode pixels or write files under the transaction.

The accepted storage checkpoint below validated a full bounded graph at start
and again at pre/final commit, with body hashing/allocation under owner access up
to the existing 32 MiB graph limit per proof. Per-dependency receipt admission was
separately fixed to read one row.

## Transaction-bound proof reuse checkpoint (2026-10-08)

Self-echo uses one `with_applied_transaction` callback. It retains an opaque
`PublishedGraphProof` borrowing that actual transaction, along with the raw root,
ordered dependency bodies/contexts and exact upload/commit receipts collected in
the existing validation pass. There is no second snapshot hash/capture pass and
no proof transfer to another transaction. A consuming `AppliedAdvance` is bound
to the same transaction, scope, prior cursor and exact next staged change. Missing
advancement, changed staged identity/cursors or any callback error rolls back
association/head/ledger/cursor writes together. The outer boundary verifies the
advance and performs only reads before COMMIT; compatibility incoming APIs remain
unchanged.

After provisional owner/receipt/cursor writes, the proof checks exact SQLite
bytes, row identities, complete membership/count/order and unchanged schema. It
rejects trigger mutation even when altered JSON remains semantically equivalent.
The final staged/ledger/head, retained current/archive and live session/selection
fences remain mandatory. This removes duplicate ciphertext extraction, JSON
parsing and hashing within that commit transaction; SQLite still compares up to
32 MiB of retained bytes under owner access. The separate preparation transaction
continues to establish its own proof. This is not a claim of zero allocation,
zero owner cost, measured responsiveness or product QueueDocument acceptance.

The existing self-echo regression exercises missing-advance rollback,
changed/deleted/replaced graph rows and schema/receipt mutation, and a real SQLite
cursor trigger changing raw dependency context after the first proof. With the
sole assigned check slot, jobs=2, locked/offline dependencies and the compatible
warm benchmark target, the two self-echo cases passed in 0.15s (10.77s compile).
Omitting only `proof.revalidate()` produced the expected RED at the post-cursor
trigger assertion: 1 failed case in 0.08s (2.35s compile). The exact reviewed
source was restored byte-for-byte before the affected incoming module passed
all 12 cases in 0.44s (1.17s compile). The three existing graph cases passed in
0.52s (4.57s compile). Strict all-target sync/native-authority Clippy passed in
1.99s, and service/sync-native Clippy passed in 7.75s. The commands are the same
focused commands shown below; the mutation ran only the exact self-echo case.

The service test artifact was `mdreader_service-7f0e66053bcfb897`; the sync graph
artifact was `mdreader_sync-049799ec158f3392`. There were no unexpected compile
failures, dependency/lock changes, model runs, packaging, deployment or live
accounts. Actual actor HTTPS acceptance remains the separately recorded prior
checkpoint, not a newly rerun claim. This proof checkpoint does not expose a
QueueDocument RPC/UI or measure owner responsiveness.

## Focused checkpoint evidence (2026-10-08)

Two real temporary-SQLite/filesystem tests passed: whole association/graph/
nonce rollback plus reopen with byte-identical ciphertext and future-namespace
preservation; strict receipt tuple/embedded-operation identity, absent-receipt
refusal, late selected fence rollback, same-document apply/reconcile, unchanged
native asset IDs/pins/line and dirty pin-ABA refusal. These use private storage
seams and synthetic keys/receipts, so they do not establish verified product login
or actual queue-through-network acceptance.

With the assigned sole check slot, `CARGO_BUILD_JOBS=2`, `--locked --offline` and
the existing warm benchmark target, the following commands ran:

```sh
cargo test --locked --offline -p mdreader-service --features sync-native --lib sync::incoming::tests::outgoing:: -- --nocapture
cargo test --locked --offline -p mdreader-service --features sync-native --lib sync::incoming::tests -- --nocapture
cargo test --locked --offline -p mdreader-sync --features native-authority --lib store::graph_tests:: -- --nocapture
cargo clippy --locked --offline -p mdreader-sync --features native-authority --all-targets -- -D warnings
cargo clippy --locked --offline -p mdreader-service --features sync-native --all-targets -- -D warnings
```

The two new cases passed in 0.16s (10.58s compile). Temporarily omitting only the
self-echo final caller fence produced the expected RED: the late `Inactive`
rejection was ignored and the transaction otherwise committed (0.07s execution,
2.51s compile). The exact file was restored byte-for-byte. The affected incoming
module then passed all 12 cases in 0.48s (2.98s compile), and the three existing
graph cases passed in 0.53s. Strict all-target sync/native-authority Clippy passed
in 2.02s; strict service/sync-native Clippy passed in 5.94s. No compile failures,
new dependencies, lock drift, model runs, packaging or live accounts were involved.
The compatible cross-worktree cache's sync input timestamp was refreshed before
Clippy to avoid relative dep-info alias reuse; no source bytes or cache were removed.

The subsequent [actual actor checkpoint](../../accounts/acceptance/README.md)
proves native-only public coordinator selection, authenticated local HTTPS
transport/self-echo and retained queue cancellation/freeze using synthetic OIDC
and the production vault codec. It does not expose QueueDocument RPC/UI or prove
live issuer/OS-keychain acceptance. Restart/login reauthorization of a queued old intent, outgoing user-state
and deletion operations, graph cleanup and history maintenance remain explicit
later work; this checkpoint never guesses those policies.
