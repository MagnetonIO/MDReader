# Native incoming owner coordinator

`sync-native` opts the service into native sync authority and core image decoding.
Default service/core builds do not acquire the decoder through this module.
This is a service owner API, not renderer RPC or shipping UI acceptance.

Native composition constructs **one** `IncomingCoordinator` per live
`Arc<Mutex<ReaderStore>>`, with explicitly selected document/asset directories and
the trusted setup's **shared remote sync library UUID**. The constructor stores
that immutable remote binding transactionally on the actual owner connection.
It is distinct from the provider registry's local profile UUID. Existing different
bindings, future schemas and malformed namespaces are rejected without reset.
Cross-library adoption/switching needs a separate explicit lifecycle.

Keep the actual `NativeAuthority` alive, call `select(&authority)`, then borrow a
privately constructed `mdreader_sync::incoming::PreparedIncoming` through
`apply(&selection, &prepared)` on a bounded blocking worker. Keep the opaque input
for reconciliation after an uncertain reply. Selection rotates a
fresh UUID even for the same library. `clear_selection` invalidates prior handles.
The handle is opaque, non-cloneable and bound to this exact coordinator/owner.
There is no plaintext constructor, authentication boolean or automatic grant.

An operation try-lock spans reservation, file writes, decoding and publication.
Authority/selection read barriers and the actual owner's try-lock are held only
for short SQLite/root/metadata work. Copying, hashing and pixel decoding run
outside those locks. No network, OS credential calls or model calls occur here.
Reopening the owner requires a fresh coordinator and asset runtime.

Only exact staged, next-contiguous changes may advance the applied cursor.
Remote UUIDs remain separate from native document/source identities and raw UTF-8
BLAKE3 revisions. Body bytes and exact title/source SHA256/selected asset identity
are preserved, including on identical-body content revisions.

## V2 history and current heads

Ordered `002-head-history.sql` upgrades only this namespaced service ledger to
version 2. It transactionally rebuilds the existing item table to replace the
one-item uniqueness constraint with per-revision/per-sequence immutable history.
Every original identity/origin/receipt string and row column is copied unchanged.
The separate `native_incoming_heads_v2` table associates scoped wire items with
kind, current wire revision, native document ID, target item, nullable baseline
generation and observed tombstone. It is wire association metadata, not another
native document, asset, source grant or filesystem authority.

Known v1 applied content rows acquire heads with **unknown (`NULL`) state
baselines**, never inferred from today's pin/line or reconstructed edits. Reserved
rows retain their exact origins. A failure rolls the whole migration back; future
versions or malformed layouts are refused without reset. An explicit baseline
reconciliation/export decision is required before modifying migrated heads.
Neither a receipt replay nor a current-file observation silently establishes it.
Global library/index/asset schema versions remain unchanged by this service slice.

At most 4,096 operation rows/heads are retained; identities are bounded to 16 KiB,
origins to 1 KiB and receipts to 32 KiB. A reserved row stores full origin/identity
before core reservation or file work. Same-live-origin retries deduplicate; changed
session/selection origins return `reauthorization_required`. Ordinary local import
recovery never activates them. No automatic cleanup, discard, rebinding, history
pruning or downgrade migration is exposed.

## Domain transitions

- New content requires no existing head and no parents. Replacement requires the
  one exact current content-head parent and a clean known native generation.
  Divergent heads, local state edits and unknown v1 baselines hold the cursor.
  The first checked head snapshot is retained through preparation; a later
  reread cannot replace it with newly changed authority. Final whole-head CAS
  repeats kind/tombstone/baseline/parent admission before publication.
  Core publishes a fresh native immutable revision and retires the previous
  document/source/assets in the same owner transaction. Pin is preserved; the
  new reading position starts at line 1 even when raw Markdown hashes are equal.
- User state has a separate wire item targeting the explicitly associated content
  item. Its parent must match the current state head (or be empty for first state).
  Exact `line` positions require the current **wire content revision**, then core
  checks actual native line bounds. `block-pixels` and stale/out-of-range positions
  stay typed pending; no approximation or revision conversion occurs.
  The codec requires a `position` key: omission is rejected before a private
  `PreparedIncoming` can exist. Explicit `position:null` requests line 1; it is
  not interpreted as a pin-only patch. Generation CAS detects pin/permission ABA.
- A known content tombstone is durably observed in its scoped head before native
  transition selection, including conflicting-parent/unknown-baseline/dirty-state
  cases. A held applied cursor cannot erase this anti-resurrection fence. A clean
  deletion atomically retires native authority and records its receipt/cursor;
  original Markdown/assets/pins/positions remain retained and explicitly exportable.
- A known user-state-item deletion tombstones that state lineage only. It preserves
  the current content document, pin and line unchanged, without granting or
  resetting them. A deleted state lineage cannot silently resurrect. Unknown
  opaque item tombstones remain unresolved; their type is never guessed.

All retained native documents/assets continue consuming the existing quotas. No
replacement/deletion overwrites or automatically prunes originals. The portable
archive API remains explicitly selected, without source regrant; retired archive
path relocation after an iOS container move is a separate core lifecycle gate.

`Receipt` keeps the original content fields and adds explicit operation, optional
generation and optional retirement. Original v1 receipts deserialize with an
unknown generation/content operation and remain byte-for-byte preserved in
history. State/deletion receipts describe the exact committed native transition;
they are not proofs of present-day read permission or new content indexing.

Core publication, item association/receipt and applied cursor share the borrowed
owner transaction. The final check rereads exact staged metadata/cursor and
receipt and scoped heads, revalidates the matching core state/replacement/retirement
source/document/assets/root/cache/file evidence, then
checks the real session immediately before commit. Any failure rolls all authority
and cursor writes back. Index work is queued **after durable commit**; queue failure
leaves an applied receipt with `index_queued:false`. Exact replay/reconciliation
repairs enqueue without republishing or reviving a disabled source.

On an uncertain result, retain the verified selected input and call
`reconcile(&current_selection, &prepared)`. A newer valid selection may inspect a
completed old receipt; it cannot rebind a reserved origin. A receipt describes
historical durable application, not current source access or successful indexing.

The focused fixtures exercise real SQLite/filesystem/core publication using the
public encrypted conformance payload, deriving explicitly referenced Markdown and
a valid PNG CRC and re-encrypting the resulting exact resources. The original
crypto vector's unreferenced/structural-only PNG is not natively publishable.
The tests call private storage seams with fixed
test fences, proving orchestration and rollback, **not end-to-end issuer identity**.
Actual `ProjectionPermit`/SessionController tests belong to sync; live OIDC/native
composition, product dispatch/UI acceptance, explicit baseline/conflict resolution,
discard/reauthorization, archive relocation and Windows incoming publication remain
separate gates. These tests do not establish actual NativeAuthority identity flow.

## Preceding new-content checkpoint

Using the existing warm target and `CARGO_BUILD_JOBS=2`:

- `cargo test --locked -p mdreader-service --features sync-native --lib sync::incoming::tests -- --nocapture`: five passed, 0.10s execution.
- Temporarily omitting only final `revalidate_publication` produced the expected
  single-test failure at “final core check must see SQL following publication”
  (0.05s). Restoring it returned the same five tests to green.
- `cargo clippy --locked -p mdreader-service --features sync-native --all-targets -- -D warnings`: passed, 7.14s.

These checks cover private-seam storage orchestration. They do not establish
native command/UI wiring, live authentication or complete incoming lifecycle.


## V2 scoped checkpoint

Four additional cohesive SQLite/filesystem fixtures cover lossless v1 migration
with transactional rollback/reopen/unknown baseline; replacement history and final
head mutation rollback; exact line/null state, generation ABA and state-item
removal; clean/dirty observed content tombstones, replay and retained archival bytes.
The five original storage/owner/session-origin/contiguous/schema cases remain.
All fixtures use privately prepared encrypted payloads and private orchestration
seams; they do not fabricate a production NativeAuthority or test issuer identity.

With the sole assigned heavy slot, `CARGO_BUILD_JOBS=2`, `--locked` and the existing
warm benchmark target:

```sh
cargo test --locked -p mdreader-service --features sync-native --lib sync::incoming::tests -- --nocapture
cargo clippy --locked -p mdreader-service --features sync-native --all-targets -- -D warnings
```

Observed nine tests passed (0.30 s). Deliberately omitting only the final replacement
head verification produced the expected RED at `apply(next).is_err()` (0.09 s): a
post-cursor trigger mutated the head and otherwise committed. The guard was
immediately restored; all nine passed (0.31 s). Strict Clippy initially found two
local style issues; after narrowing the private state plan to a pin and optional
line, strict Clippy passed (3.63 s) and the affected exact-state/null/ABA/deletion
lifecycle recheck passed (0.10 s). No broad suites, models, app build, dependencies,
lockfile drift, installed profile or live provider/authentication access occurred.


Static review found that the initial replacement preparation reread could adopt
changed head authority after its first admission check. The corrected flow carries
the first checked snapshot through preparation and repeats content admission at
commit. The existing replacement fixture now also inserts a reservation-trigger
head/tombstone mutation between those checks. The corrected final tree passed
the exact replacement lifecycle fixture (one test, 0.10 s execution; 2.55 s build)
and strict service feature all-target Clippy (2.56 s). The assigned build slot was
released immediately afterward. These are storage/orchestration checks, not
product UI or live identity acceptance.
