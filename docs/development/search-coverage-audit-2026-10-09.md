# Search coverage audit

Read-only audit by backend at `7d051517`; relevant search/index source is unchanged
in picker integration `6e73051`. No private-profile query or indexing run was used.

## What the current source establishes

- `src-tauri/src/search.rs:136–305` builds the complete selected authorized query
  scope and revalidates the retained root and owned source receipts after search.
  `crates/mdreader-service/src/device/search.rs:4–35` pages current device entries.
- `crates/mdreader-core/src/store.rs:686–722` commits an eligible local scan
  atomically. Failed refreshes can leave the previous snapshot. Eligibility is
  `.md`/`.markdown` up to 2 MiB, subject to ignore rules and hidden/symlink bounds;
  it is not every file on the machine. `.mdx` is disabled.
- Provider metadata completeness is a separate observation. Registry and
  projection admission join bodies to the enabled binding and current exact
  revision before indexing; metadata-only rows are not content-searchable.
- Existing index status counts available documents/chunks/embeddings; source jobs
  expose generation, phase and error. Catalog snapshots expose committed identity
  and observation time. These do not all reach the search palette.
- `src-tauri/src/engine/index.rs:69` currently assigns `files_total = files_done = job.documents`.
  This is a committed count, not a discovered eligible denominator.
- `src-tauri/src/engine/navigation.rs:34–61` uses `fromIndex` as listing-origin information, not a
  completeness proof. `SearchResults`/renderer `SearchOutcome` expose hits and
  semantic unavailability. Model readiness is distinct from full vector coverage.

## Smallest truthful contract

Add an owner-generated, bounded `SearchCoverage` alongside outcomes, scoped and
revalidated through the same authority as returned hits. Include an opaque scope
token and one record per admitted source, capped to that actual query scope:

- Source kind/opaque identity, lexical indexed document/chunk counts, source job
  phase, committed snapshot and observation time.
- Semantic model state and embedded chunk count, separate from indexed chunks.
- Optional provider metadata state, known selected documents and admitted bodies,
  computed against the exact binding/snapshot. Use null when no census exists.

Do not expose private paths, credential material or per-file error text. Do not
call metadata-only rows pending-index work. Current job records do not establish
per-document pending/failed counts; report a source refresh failure and unknown
denominator instead of inventing either. Device counts must join current receipts
and revisions to the derived index rather than trusting a list-count heuristic.

The UI can say “N Markdown files indexed”, “Refresh pending/failed”, or “Indexed
at [observation]”. A successful atomic local scan can establish completeness at
that observation, with explicit eligibility and scope. Counts alone cannot
establish current filesystem completeness or “all files searchable”.

## Verification required

Use synthetic real SQLite and files: a successful multi-directory eligible scan,
ignore/size boundaries, refresh interrupted/failed with old snapshot retained,
revision changes/revocation, provider metadata without bodies, admitted current
bodies, lexical readiness before model loading, and partial/full embedding.
Inspect actual content-only fixture queries and returned scope/snapshot fencing.
This audit is source evidence and a contract proposal, not implemented coverage
DTOs or acceptance of the user's current private folder.
