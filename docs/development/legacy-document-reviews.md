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
