# Offline legacy snapshot archive v1

This CatDB-free library/tool preserves **explicitly selected, already-consistent
offline snapshots**. It does not discover profiles, initialize a legacy SDK,
crawl sources, read a vault, activate a profile, or import into `reader-v1`.
Inputs and destinations containing a `reader-v1` path component are refused.
Task 7 migration and permission to delete legacy data are not complete.

## API and tool

`export(&[Input], destination, &Limits)` returns a `Manifest` only after durable
publication. `verify(archive, &Limits)` checks the bounded manifest, exact member
set, file privacy and every member's length/SHA-256. `SnapshotSpec::from_inputs`
builds an explicit versioned selection; `read_spec` reads that bounded selection.
The tool takes native OS arguments and prints a small receipt without contents:

```text
mdreader-legacy-export export ABS_SPEC_JSON ABS_DESTINATION
mdreader-legacy-export verify ABS_ARCHIVE
```

The specification is JSON `{ "version": 1, "offlineSnapshots": true, "inputs":
[{ "role": "proposals", "locator": { "encoding": "unix_bytes", "units":
[47, 116, 109, 112, 47, 112, 46, 100, 98] } }] }`. Use the Rust constructor to
encode actual selected paths; native units preserve non-UTF-8 Unix paths and
Windows wide paths without lossy display conversion. Restoring a different
host's encoding is unsupported. Accepted roles are `proposals`, `catalog`,
`cloud_catalog`, `library`, `settings`, `session` and `enterprise`.

The destination's existing canonical parent must be private (Unix mode has no
group/other bits). Its new basename must contain 1–64 ASCII letters, numbers,
underscores or hyphens, beginning with a letter/number. Existing destinations
are never replaced. Inputs must be absolute regular files; leaf symlinks,
duplicate file identities, SQLite `-wal`/`-shm`/`-journal` sidecars and changed
input identities/bytes are refused. Source descriptors remain open; identity
and SHA-256 are checked before copying and again after conversion and before
publication. SQLite queries open only the private copied snapshot, read-only
and immutable; input databases are never opened with SQLite or an SDK.

These checks cannot prove that another process is idle or make independently
selected files transactionally consistent. The caller must supply snapshots
from an offline or owner-coordinated snapshot operation. This first slice does
not implement that operation. `offlineSnapshots: true` is required by the tool,
and is an explicit caller assertion rather than discovery evidence.

## Preserved format

`manifest.json` describes generated `0000.raw` members, optional
`0000.records.jsonl` members, original native locators and conversion status.
The raw member is authoritative exact bytes, preserving unknown rows, schema,
JSON fields, historical records and rejected-review tombstones. Typed records
are an interpretation for a future importer, never a replacement for raw data.
SHA-256 detects corruption against the manifest; it does not authenticate an
archive if someone can replace both contents and manifest.

Known conversions recognize proposal-store SQLite versions 1/2, library JSON
versions 1–9 (`sources`), session-store version 1 (`session.tabs`) and settings
store objects (`settings`). Proposal rows keep every column and actual rowid.
SQLite cells use tagged null, decimal-string int64, hex IEEE-754 real bits,
hex text bytes and hex blob bytes; version 1 does not invent `reviewed_hash`.
No historical events are reconstructed. Unknown schema/version or malformed
known JSON is explicitly unavailable while its exact raw snapshot remains.
Catalog/cloud/enterprise semantic conversion remains unavailable.

SQLite metadata includes schema hash, user version and physical integrity-check
status. Virtual-table row counts are `null`: loading arbitrary legacy modules
is not supported. A physical `integrity_check` success is not logical virtual
table coverage. A missing-module integrity check is explicitly marked
`unsupported_virtual_module`; other corruption/errors fail export. Raw virtual
schema/shadow data is retained even when semantic conversion is unavailable.

No credential role or credential-store adapter exists. Do not select credential
files. Selected legacy authored snapshots are private data and may themselves
contain embedded secrets in arbitrary text/unknown fields. A lossless raw export
cannot certify their absence or redact them without changing the preserved
bytes. Archives remain local and private; no upload/sharing is implemented.

## Bounds, publication and recovery

Hard ceilings are 128 artifacts, 256 MiB per raw artifact, 1 GiB total members
plus manifest, 8 MiB JSON input, 1 million rows per physical table/conversion,
4 MiB per schema/typed row, 256 MiB per typed member and 512 KiB manifest. A
caller may lower bounds, never raise them. Raw files/hashes stream through a
64 KiB buffer; SQLite typed rows stream one at a time. Oversize input fails
without truncation or a completed receipt.

Unix publication uses descriptor-relative 0700 staging/0600 files, synced files
and directories, an atomic no-replace rename, then parent-directory sync.
Failure or process death can leave a private `.mdreader-export-UUID` staging
directory. It is not a completed archive and `verify` refuses that path. Retry
uses a fresh staging directory; no existing completed archive is overwritten.
Staging cleanup is an explicit owner action, not automatic recursive deletion.
If the process fails after rename but before receiving a receipt, verify the
selected destination before deciding whether a new export is needed.

macOS publication and actual process interruption are tested. Linux uses the
same Unix implementation but has not been exercised in this acceptance run.
Windows fails closed pending ACL and durable no-replace publication support;
other Unix hosts without the implemented no-replace primitive are unsupported.
No multi-file activation, import, credential reconnection, rollback switch or
installed-profile rehearsal is included. Existing source snapshots and new
reader state remain the rollback authority.

## Archival document reviews

The `reviews` module provides `stage_reviews(archive,
expected_identity, NEW_destination, limits)` and read-only
`ReviewArchive::{open,list,get,raw_record}`. It revalidates the selected
export/manifest identity and re-derives typed proposal records from raw SQLite
before one transaction imports exact rows into a separate `reviews.sqlite`.
The private completed stage is idempotent for that exact archive identity;
it cannot replace reader state or a different completed stage.

Known decision/architecture assertions become archival span candidates;
unsupported or malformed assertions and unknown review states remain archived.
Reviewer, rejected tombstones and v1 missing-review-hash semantics are preserved.
Current graph membership, review chronology and current-source evidence are not
available. The completed raw export must remain retained for unknown tables and
schemas. See the [schema/read contract](../../docs/development/legacy-document-reviews.md).

The CLI exposes the same separate staging/read boundary:

```text
mdreader-legacy-export stage-reviews ABS_ARCHIVE EXPORT_UUID MANIFEST_SHA256 ABS_NEW_DEST
mdreader-legacy-export list-reviews ABS_STAGE EXPORT_UUID MANIFEST_SHA256 OFFSET LIMIT
mdreader-legacy-export get-review ABS_STAGE EXPORT_UUID MANIFEST_SHA256 RECORD_ID
```

Use the export UUID returned by `export`/`verify` and the SHA-256 of the explicitly
selected archive's `manifest.json` (for example, `shasum -a 256
/ABS/ARCHIVE/manifest.json` on macOS). UUID and hash are mandatory on every review
command; read commands never infer them from a staging directory. Hex arguments
are canonicalized to lowercase. From the repository root, the pinned Rust
toolchain can run the tool with `cargo run --locked -p mdreader-legacy-export --
COMMAND ...`; it requires no GUI, SDK initialization, provider login or model.

Start listing at offset `0`, choose limit `1`–`100`, and use the returned
`page.nextOffset` until null. `get-review` uses a `recordId` from that page and
returns `record: null` if it is absent. Both responses have `scope:
archival_only`, `currentDocumentEvidence: unavailable` and `sourceBinding:
not_performed`. The record's assertion discriminant distinguishes an internally
consistent **archival** span candidate from an unavailable interpretation; neither
is a present-day citation. Exact tagged raw rows remain separately available
through `ReviewArchive::raw_record` and the retained original archive, and are
never automatically included in read-command output.

Receipt output is capped at 4 KiB; list/get JSON is capped at 4 MiB plus 4 KiB
for the envelope and terminator. Serialization is bounded and completes before
stdout begins. Validation failures exit nonzero with empty stdout and sanitized
errors that do not echo arguments, paths, reviewer identities or row contents.
Review commands perform no activation, source binding, grant or graph action.

## Portable current-revision comparison boundary

The `binding` library module accepts one explicit `SelectedBinding` (archive
identity, record ID and current owner/source/path/exact revision) and a trusted
`CurrentDocumentReader`. It reinterprets the raw archived row and compares its
inclusive normalized line span with a bounded current body, then requires final
owner and archive revalidation. Recorded/current and reviewed/current hashes
remain separate; absent reviewed hashes stay unknown. Unsupported schemas,
rules or hashes cannot become matches. Archives and owner state are unchanged.

This is an injectable portable boundary, **not native reader integration**.
The trait defaults to unsupported. A later adapter must borrow the selected
concrete ReaderStore, preserve enabled/current-generation/exact-revision gates
and use a stable authoritative profile UUID. Local raw revisions are checked;
provider revisions are opaque and labeled as cached observations, not remote
head freshness. No profile discovery, inferred grant, fuzzy relocation,
approval migration, graph membership or CLI binding command is supplied.
See the [producer semantics and adapter requirements](../../docs/development/legacy-document-reviews.md#explicit-current-revision-text-comparison).
