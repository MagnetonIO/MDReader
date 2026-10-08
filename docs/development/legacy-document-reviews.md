# Archival document insights and reviews

This Task 7 slice reads a verified, explicitly selected offline export into a
**new standalone staging destination**. It has no CatDB dependency, source
grant, active-profile pointer, review transition, graph write or SDK initialization.
The completed raw export remains authoritative and must be retained. Staging
does not authorize deleting it or claim that useful legacy behavior is migrated.

## Verified legacy meaning

[`facts_of`](../../crates/mdreader-ingest/src/proposals.rs#L734) records an object
or morphism assertion, SQL citation columns and the complete serialized payload.
[`persist_review`](../../crates/mdreader-ingest/src/proposals.rs#L539) updates SQL
state/reviewer/reviewed hash without rewriting payload provenance. Consequently
an approved SQL row can still have `payload.provenance.approved_by = null`.
Both are preserved, without fabricating an approval stamp in the payload.

[`approve_and_promote`](../../crates/mdreader-ingest/src/proposals.rs#L626)
writes graph first and proposal store second. Re-import can reset an approval
without retracting a graph fact; rejection tombstones survive disappeared facts
and changed cited text. [`inspected`](../../src-tauri/src/overlay.rs#L1024)
independently reads graph membership. Therefore archived `approved` is a recorded
review state, never evidence of current or historical graph membership. The
proposal table has no complete timestamped event history; this slice records
chronology and graph membership as **unavailable**, never `false` or invented
events. A v1 row without `reviewed_hash` remains unknown at that field.

## Representable assertions

Document-span candidates have a valid UUID, known state, the exact known
schema/extractor/rule/entity combination below, a nonempty UTF-8 opaque legacy
source locator, a valid inclusive 1-based range and `blake3:<64 hex>` citation.
Payload ID/name/endpoints and provenance source/range/hash/extractor/schema
version must agree with the SQL columns. Confidence must be finite in `[0,1]`.
This validates an archival record's internal consistency, not its truth.

| Schema / extractor | Object rule → entity | Morphism rule → arrow |
| --- | --- | --- |
| `decision_overlay` / `mdreader.decision@1` | D1/D2 → Decision | D3 → supersedes |
| `architecture_overlay` / `mdreader.architecture@1` | A1 → Component; A2 → Layer; A3 → Interface; A4 → Dependency | A2 → inLayer; A3 → exposes; A4 → dependant/provider/via |

These are the actual mappings in [decision extraction](../../crates/mdreader-domain/src/extract/decision.rs#L105)
and [architecture extraction](../../crates/mdreader-domain/src/extract/architecture.rs#L145).
Unknown states are `unknown`, never normalized to proposed. Generic catalog,
administration, unsupported schema/rule/version, malformed UUID/payload and
nonrepresentable spans remain archived with an unavailable interpretation.
Every exact exported row is retained regardless of interpretation.

`RecordedHashComparison` compares only the **recorded** current-extraction and
reviewed hashes: equal, differ or unknown. It makes no statement about a file's
present bytes. Valid candidates always have `currentEvidence:
source_binding_required`. A future owner must explicitly bind the archived
locator to an admitted source generation/document revision and verify the exact
cited bytes before exposing current reader evidence or offering a new review.
No locator, endpoint or object ID in this archive grants source access. Even
absolute/traversal-shaped locator text is opaque and is never opened or joined
to an inferred root.

## Staging schema and identity

Input `ArchiveIdentity {exportId, manifestSha256}` is selected explicitly. Before
staging, verify the bounded archive's exact member set/hashes and the selected
manifest identity. Re-derive proposal JSONL from its raw snapshot and compare
its hash/count with the exported typed member before ingesting a single row.
This rejects typed/raw disagreement even if a caller supplies a newly hashed
manifest. Hashes establish consistency with the selected identity, not author
authentication.

One transaction in a new `reviews.sqlite` writes namespaced tables:

- `legacy_review_schema_v1`: its own version guard; no global `user_version`.
- `legacy_review_imports_v1`: exact export/manifest identity and staged row count.
- `legacy_review_artifacts_v1`: artifact index, raw/typed member identity and
  conversion availability; unavailable artifacts are visible in the receipt.
- `legacy_review_records_v1`: stable record ID, artifact index, original int64
  rowid, exact tagged JSON bytes/hash and the bounded archival interpretation.
  `(artifact_index, legacy_rowid)` is unique; fact IDs are not deduplicated across
  source artifacts, so distinct occurrences survive.

`RawReference` records export/manifest identity, artifact index, raw member/hash,
typed member hash, decimal-string rowid and exact row SHA-256 (excluding its
JSONL terminator). Unknown columns and payload bytes survive in `raw_record`;
unknown tables/schema remain in the separately retained completed raw export.
The SQLite transaction contains no source, grant, graph or review-event tables.

After commit/close, sync the database, publish a bounded manifest with its hash,
then use private no-replace staging publication. No completed destination is
overwritten. Exact retries validate the existing stage and return `replayed`;
different selected identities conflict. Interrupted staging is not readable as
a completed archive and retry uses a new private stage. There is no merge into
`reader-v1`, automatic activation or rollback switch.

## Read contract and limits

`ReviewArchive::open(stage, expectedIdentity, limits)` verifies the completed
stage and opens SQLite read-only/immutable. `list(offset, limit)` returns bounded
`ArchivedReview` values and an explicit next offset; `get(recordId)` reads one
interpretation; `raw_record(recordId)` reads its exact tagged bytes. Each review
includes the raw reference, preserved current state/reviewer/hash fields,
recorded-hash comparison and either a document-span candidate or unavailable
reason. Graph membership/chronology remain unavailable for every row.

Use the exporter hard ceilings: 128 artifacts, 1 million rows, 4 MiB input row,
256 MiB staged SQLite file, 512 KiB manifest; callers may lower them. Read pages
accept at most 100 rows and at most 4 MiB serialized records, with explicit
pagination rather than silent loss. Raw-record reads remain bounded separately.
Windows is fail-closed pending private ACL/durable publication support; no live
profile or application/service/core integration is part of this slice.

The standalone CLI now exposes `stage-reviews`, `list-reviews` and `get-review`
with explicit export UUID and manifest SHA-256 on every invocation. See the
[tool commands and output bounds](../../crates/mdreader-legacy-export/README.md#archival-document-reviews).
Read output explicitly labels current-document evidence unavailable and binding
not performed; record discriminants preserve the candidate/unavailable distinction.
Raw tagged rows remain separate from interpreted JSON responses. The CLI does
not change the authority or retention requirements above.

## Explicit current-revision text comparison

The portable `binding` module compares one selected staged record with one
explicit current owner/source/path/revision. It does **not** implement a native
ReaderStore adapter, CLI binding command, durable binding, source grant, review
transition or application integration. An injected test reader is only the
isolated authority boundary, not evidence that the native path is integrated.

The producer contract is specific:

- [`source_document`](../../crates/mdreader-ingest/src/source_view.rs#L57)
  hashes `str::lines()` joined with LF. CRLF becomes LF and a terminal newline
  is omitted. Section citations hash their own inclusive span, not a nested
  subtree or the parser's structural section hash. Link assertions cite a
  single line. The recorded range determines exactly which lines are compared.
- [`persist_review`](../../crates/mdreader-ingest/src/proposals.rs#L539) stamps
  `reviewed_hash = content_hash`. Later extraction can change the recorded hash
  while retaining a rejection and its earlier reviewed hash. Missing v1 review
  hashes remain unknown; the comparison never substitutes the recorded hash.
- [Ingest decoding](../../crates/mdreader-ingest/src/ingestor.rs#L385) used
  `String::from_utf8_lossy` after a separate raw-byte change-detection hash.
  Normalized citation equality cannot establish equality of legacy raw bytes.
- Native [`read_document`](../../crates/mdreader-core/src/store.rs#L755)
  reads strict UTF-8 at an enabled admitted source and checks the selected exact
  revision. Local revisions hash raw bytes; provider revisions incorporate the
  remote version and body hash. A provider revision is opaque, not a raw-body
  hash. [Provider reads](../../crates/mdreader-core/src/provider_projection.rs#L375)
  revalidate current generation, cached body hash and revision. They do not
  establish freshness against the remote provider head.

`SelectedBinding::new` requires the selected archive identity and record ID plus
`CurrentDocumentReference::new(ownerScope, sourceId, relativePath, revision)`.
UUIDv4 owner identity, bounded source/path, normal relative path components and
64-hex exact revision are validated without resolving a filesystem path. The
legacy locator is never opened or matched to discover a document. Selecting a
renamed document is an explicit caller choice, not an inferred relocation.

`verify_binding` reads the exact raw staged proposal, checks its row hash and
archive/record identity, and reinterprets the known schema/rule/version from
that raw record. Unsupported assertions or hash encodings fail before a current
read. Only `blake3:<64 hex>` is supported; a missing/null reviewed hash produces
`unknown`, while an unrecognized non-null hash cannot produce `matches`.

The trusted reader must belong to the selected owner and return the exact
selected reference. Current UTF-8 bodies are bounded to 2 MiB. Local raw-byte
revisions are also checked against the returned body; provider revisions remain
opaque and require the owner's exact-revision cache read. The cited lines are
hashed incrementally with the producer's normalization. Missing current lines,
including an empty document with legacy degenerate range `[1,1]`, are rejected
rather than reproducing historical clamping. No fuzzy search, extractor rerun,
endpoint resolution or relationship validation occurs.

The result separates `recordedCurrent` and `reviewedCurrent` comparisons and
labels its scope `selected_owner_revision`. The original archival state,
reviewer, raw reference, unavailable chronology/graph membership and
`source_binding_required` label remain unchanged. A `matches` value establishes
only normalized cited-text equality at that explicitly selected revision. It
does not approve an assertion, establish current semantic truth or upgrade an
archival review into a citation. `provider_cached_revision` explicitly identifies
a cached observation; it is not a remote-head freshness assertion.

Before returning, the reader must revalidate the selected revision and enabled
authority and the archive read guard must still pass. Failures return sanitized
typed errors, never a partial comparison. This is a transient observation;
later evidence use must revalidate the exact reference again. Nothing is saved
to either archive or owner.

### Native owner adapter and remaining migration gates

The desktop service now supplies the adapter in
[`mdreader-service::reviews`](../../crates/mdreader-service/src/reviews/README.md).
It borrows the existing selected concrete `ReaderStore`, with a durable UUID in
a versioned namespaced owner table. It never opens a profile discovered in an
archive. An explicit native folder picker plus export UUID and manifest SHA-256
select one verified stage. Its opaque handle is volatile; restart requires
reselection. The renderer receives no stage or original source locator.

Both initial read and final revalidation call
`read_document(source, path, Some(revision))`, retaining enabled-source,
retained-root, current-generation and provider-cache gates. Profile/stage
identity and owner UUID/version are checked before and after operations. Archive
hashing happens outside the owner mutex; one active review operation is allowed,
with typed `busy` on contention. Windows remains unsupported by the archival
stage implementation. Initialization failure is reviews-only and does not block
local reading.

The desktop reader exposes **Archived reviews…** from the local inspector and
cached provider reader. Opening the panel performs no archive selection or
current read. The user supplies the exact export identity, chooses the stage,
and explicitly compares a representable archival record with the open document.
Local selection captures the expected workspace generation through the final
response, including same-root reopen. Provider comparison uses only its exact
admitted cached reference and does not fetch the remote head. The panel escapes
archived assertion text and compared Markdown as plain text.

Comparison displays the actual inclusive current passage and exact revision,
with distinct recorded/current and reviewed/current results. It does not jump
to the latest file and claim that later bytes are the compared evidence. Unknown
interpretations and rejected tombstones remain visible. The complete raw stage
and export remain authoritative; no review transition, graph replay, approval
migration, durable source binding or artifact mutation occurs. See the
[focused verification receipt](archived-review-validation-2026-10-08.md).

This closes the explicit archival observation adapter gap. Actual packaged
native picker/dialog acceptance, installed-profile writer barrier/export,
activation/rollback, generic legacy administration migration and complete CatDB
removal remain separate gates. Existing populated live proposal behavior is
preserved. When the current SQLite indexing path has no live proposal store, the
inspector now reports unavailable and points to explicit archival review
selection instead of returning a misleading empty success.

Portable comparison checkpoint: the three real-archive/injected-owner behavior
tests failed against the unsupported stub, then passed after implementation.
The affected crate passed 14 unit tests and two CLI subprocess tests; its ignored
child helper is exercised by the actual exporter spawn/kill test. Crate-only
all-target strict Clippy passed. These checks used locked dependencies and two
build jobs on macOS. They establish the portable/archive boundary, not native
adapter integration, remote freshness or release migration acceptance.
