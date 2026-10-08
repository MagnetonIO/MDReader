# Legacy migration inventory and proposed first slice

Code inspection at `feb7a2d` on 2026-10-07, for approved
[Task 7](../plans/sqlite-reader-next.md#task-7-phase-5--vs-code-migration-and-release).
This is a proposal, not an implemented migration or a profile audit. No installed
profile, credential, private source, or provider was accessed. Line anchors below
describe this checkout; symbol names remain the primary references.

## What must survive

| Artifact / authority | Preserve and migrate or archive | Rebuildable subset / code evidence |
| --- | --- | --- |
| External Markdown and assets | Original bytes, native path encoding, admitted filesystem identity; files are never moved or rewritten by migration. Missing/offline sources remain unresolved records. An export of app state is not a backup of external content. | Rebuild search from explicitly revalidated roots. [`NativePath`](../../src-tauri/src/library.rs#L225) retains Unix bytes / Windows wide units; [`LocalFolderSource`](../../src-tauri/src/library.rs#L520) carries stable engine UUID and root identity. |
| `library.json` (currently version 9) | Raw bytes plus all memberships, active source, restore suppression, source contracts, cloud selection/auth profile, Connected CatDB selection/pins, rediscovery requirement and cleanup intents. Preserve unknown JSON fields in the archive even if a converter cannot interpret them. | Availability and index counts are observations, not grants. [`StoredLibrary`](../../src-tauri/src/library.rs#L637), [`PendingCleanup`](../../src-tauri/src/library.rs#L782), [`save_to`](../../src-tauri/src/library.rs#L1709). |
| `workspaces/<stable-identity-key>/proposals.db`, and recoverable legacy workspace directories | Every `proposal` column, including payload, fact ID, extractor/rule/schema, endpoints, source span/current hash, review state/reviewer and `reviewed_hash`. Keep rejected/deleted-source tombstones and orphan rows. | New extraction may regenerate suggestions but cannot regenerate a person's decision. [`DDL`](../../crates/mdreader-ingest/src/proposals.rs#L296); retained tombstones and changed-review rules are documented at the [module boundary](../../crates/mdreader-ingest/src/proposals.rs#L40). [Workspace paths](../../crates/mdreader-core/src/source_root.rs#L850) and [V1 quarantine](../../src-tauri/src/library.rs#L2097) include `legacy-v1-path-workspaces`; cleanup tombstone locations also need explicit inventory. |
| Workspace `catdb.db` | Snapshot the entire database until user-authored graph/catalog/log records have been classified and archived. Approved objects/morphisms, provenance, immutable schema/mapping/source/policy catalog versions, operation envelopes and chain anchor are not disposable simply because the filename is called an index. | Document scans, sections, chunks, links, FTS/vector rows and embedding-version state can be rebuilt. [`open_engine`](../../src-tauri/src/engine.rs#L866), [`approve_and_promote`](../../crates/mdreader-ingest/src/proposals.rs#L626), [`reject_and_retract`](../../crates/mdreader-ingest/src/proposals.rs#L663). See pinned SDK evidence below. |
| `cloud-library.catdb` | Snapshot the mixed CatDB catalog and every `mdreader_cloud_*` table. In particular preserve accepted-write repair IDs, revisions, state, body hashes and receipt bytes: the receipt contains the user's full Markdown content. Keep cached bodies as offline observations, even when remote content is unavailable. | Published/staged metadata and cursors can be resynchronized only after fresh selected-source authorization; cached bodies are not proof of current head. [`configure_catalog`](../../src-tauri/src/cloud_accounts.rs#L1923), [table definitions](../../src-tauri/src/cloud_documents/store.rs#L28), [`CloudWriteReceiptPayload`](../../src-tauri/src/cloud_documents/service.rs#L43). Legacy [`retire_source`](../../src-tauri/src/cloud_documents/store.rs#L1201) deletes repair rows, so it is not a migration/cleanup primitive. |
| `session.json`, `settings.json` | Exact bytes plus local tabs/history/order/pins/scroll/origin root, recents, theme/font/text size/rail visibility, lens choice and reviewer identity. Retain unrecognized records without silently defaulting them away. | [`SessionData` / serialization](../../ui/src/stores/session.ts#L15), [`SettingsSnapshot`](../../ui/src/stores/settings.ts#L67), [store names and keys](../../ui/src/shell/persistence.ts#L75). Cloud open tabs/edit authorities are deliberately not persisted; cloud recents can be present. Unsaved [editor buffers](../../ui/src/stores/editor.ts#L1) are memory-only: migration must first resolve the existing unsaved-change guard, not claim to recover them from disk. |
| Legacy enterprise files and credentials | Include explicit `mobile-enterprise.json` / `.migrated` artifacts if present, plus their registry metadata. Leave OS vault entries untouched for rollback; retain references only in the private archive. | [Legacy enterprise paths](../../src-tauri/src/mobile_enterprise.rs#L646), [enterprise vault namespace](../../src-tauri/src/mobile_enterprise.rs#L208), [cloud vault namespace](../../src-tauri/src/cloud_accounts.rs#L2302). No OS-vault secret export, keychain scan, logout, automatic reconnection, or reuse in fresh native-v3 custody. |
| Already-created `reader-v1` state | Preserve the current library, captured Markdown/provenance, source grants, provider admission tombstones, mutation/outbox/receipt/cursor state and integration ownership. Legacy import must not replace/reset newer user state or its authority history. | [Current profile location](../../src-tauri/src/lib.rs#L312); [core library/index separation](../../crates/mdreader-core/src/store.rs#L117). `index.sqlite` is derived; durable `library.sqlite` and capture bodies are not. This is protection around the legacy conversion, not legacy data to reinterpret. |

The pinned dependency is `CatDB@c06e45404a3805183ae2cead0b0ad37679d24ace`
([lock entry](../../Cargo.lock#L1352)). In that source:
`catdb/crates/catdb-storage-sqlite/src/sql.rs:691` defines immutable catalog tables;
`:798` documents ordered `operation_log` envelopes and `operation_log_anchor`.
`catdb/crates/catdb-engine/src/engine.rs:1281` verifies that chain and `:1321`
exports its JSONL. The existing export covers the log, not proposals, settings,
cloud repair receipts or the complete multi-file profile. Opening the SDK engine
or [`ProposalStore::open`](../../crates/mdreader-ingest/src/proposals.rs#L375)
can create/migrate databases; neither is a read-only discovery/export method.

## Lossless export and rollback contract

Proposed `mdreader-legacy-export/v1` is a private directory bundle, published by
durable atomic rename after all members and the manifest are synced. Generated
member names never interpret provider keys or legacy paths as archive paths.

- `manifest.json`: export UUID/schema, producing build/SDK pin, explicit selected
  artifact inventory, missing/unsupported entries, native-encoded original
  locators, per-member byte length/SHA-256 and SQLite schema/user-version/row
  counts. A completion receipt records what was actually included; limits,
  corruption or a missing required member cannot produce a complete receipt.
- `snapshots/*.sqlite`: consistent standalone SQLite snapshots, including
  unknown tables, ordered rowids and SQLite metadata. Use the owning connection's
  backup facility or verified closed-profile copies; never copy a live main file
  while ignoring WAL/journal state. Pause legacy import/review/provider/background
  writers and flush the JSON/session writer before taking a profile-wide barrier.
  Separate per-database read transactions alone do not provide that barrier.
- `raw/*.json`: exact original bytes, not a sanitized/deserialized rewrite.
  `records/*.jsonl`: bounded streaming logical records with explicit table/column
  identities and tagged SQLite values (`null`, integer decimal string, real
  IEEE-754 bits, text bytes, blob bytes). This avoids JavaScript integer rounding,
  UTF-8 replacement, opaque-BLOB loss and JSON payload normalization. Keep raw
  schema, unknown values and all rows even when a semantic adapter is unsupported.
- Optional verified operation-log JSONL plus anchor, and document-readable review
  Markdown/JSON, are convenience projections. They never replace raw snapshots.
  Archives are local private user data, not diagnostics, sync payloads or a public
  fixture; native privacy/ACL support is required on each supported host.

Review conversion must preserve state and graph presence separately. Approval
writes to the graph **before** the proposal store, so a crash can leave graph
presence with a proposed row. The existing importer also documents approval reset
without graph retraction. Do not repair either case by inventing approval.
[`row_to_proposal`](../../crates/mdreader-ingest/src/proposals.rs#L922) normalizes
unknown review states and malformed UUIDs, so raw SQL values must be exported
before invoking legacy typed readers. The current `Review` has state/reviewer,
not a timestamped complete review-event history; preserve available operation-log
events and record absent chronology as unavailable, never synthesize it.

Build the migrated library in a separate destination/profile, preserving any
already-active `reader-v1` state with an explicit merge/reconciliation plan rather
than replacing it. Commit a service-owned
import receipt, artifact hashes, legacy-to-reader source aliases and converted
user records atomically before changing the active profile pointer. Records with
missing sources, stale hashes or unsupported schema stay durably archived with
explicit availability, not silently discarded or asserted as current evidence.
Local roots require object-identity revalidation; provider sources require fresh
narrow Picker/AppFolder consent and generation binding. Never turn an old broad
grant or cached body into a new Local source.

Before cutover, rollback means deleting only an unselected staging destination.
After cutover, rollback means switching to the preserved legacy profile after
closing both owners and revalidating its grants; it never overwrites subsequent
source edits, provider bodies, new user state or either profile from an old
snapshot. New writes require an explicit reconciliation/export before a later
rollback. Accepted/unknown remote writes cannot be undone by profile selection
and must never be resent. Keep originals and the completed export until an
explicit retention decision; do not dual-write indefinitely.

## What still prevents removing CatDB from the shipped app

1. [`src-tauri/Cargo.toml`](../../src-tauri/Cargo.toml#L69) still requires the
   local-file SDK Git remote plus domain/ingest/local connector dependencies.
   Domain, ingest, presentation and local connector manifests require CatDB;
   Drive/Dropbox retain default `legacy` compatibility features. A fresh clone
   elsewhere cannot satisfy that local SDK URL. The new core/provider/service
   path being CatDB-free does not make the native application CatDB-free.
2. [The live command registry](../../src-tauri/src/lib.rs#L99) still exposes
   legacy entity/catalog/federation/schema/mapping/constraint/review and Connected
   CatDB operations. Startup still configures/schedules the legacy cloud catalog
   alongside `reader-v1` ([setup](../../src-tauri/src/lib.rs#L312)). Remove generic
   administration only after export; port useful document review/evidence APIs.
3. Local [`start_import`](../../src-tauri/src/engine.rs#L1129), listing, search and
   Related now use the service. However [legacy Library hydration](../../ui/src/shell/library-documents.ts#L254)
   still performs CatDB catalog/entity queries; Settings still exposes [Connected
   CatDB](../../ui/src/shell/SettingsSheet.tsx#L810). The new selected-provider
   panel/dialog is a shipped separate path, not completed sidebar/global-search
   migration. Root owns that cutover; provider link navigation is a separate slice.
4. Useful legacy insights have no new durable schema/importer yet.
   [`inspect_span_guarded`](../../src-tauri/src/overlay.rs#L825) returns an empty
   list if the epoch has no proposal store; [`review_target`](../../src-tauri/src/overlay.rs#L990)
   requires the legacy engine/store. Core [Related](../../crates/mdreader-core/src/related.rs#L1)
   explicitly labels knowledge/concept/insight migration gaps. Introduce bounded
   MDReader-owned document insight/review DTOs with exact span/hash provenance
   and honest unavailable/stale states before claiming useful behavior retained.
5. No owner-coordinated multi-file snapshot, durable import/activation journal,
   source-ID alias mapping, profile rollback flow or migration rehearsal exists.
   Settings/session loaders currently sanitize/drop malformed entries. New core
   bookmarks and mobile reading position are not automatic imports of old tabs,
   scroll positions or review records. Platform privacy, runtime-only credential
   reconnection and unknown-write preservation remain explicit gates.

## Small first implementation slice

Add a CatDB-free `mdreader-legacy-export` library that accepts **explicit,
already-consistent offline snapshot inputs**, exports/verifies the bundle above,
and recognizes proposal-store v1/v2 plus raw library/session/settings JSON. No
installed-profile discovery, SDK initialization, import, UI activation, vault or
provider access in this slice. Unknown schemas can be retained as raw snapshots
with semantic conversion unavailable; they cannot be declared migrated. The owner
snapshot/quiescence adapter and full mixed-catalog conversion follow separately.

Use one synthetic multi-file fixture and focused RED→GREEN scenarios: exact
reopen roundtrip of approved/rejected/changed/deleted-source proposals plus large
integers/BLOBs/non-UTF-8 paths/unknown JSON; interrupted publication and resume;
tampered/missing member refusal without a completion receipt; unchanged source
bytes; verification/rollback from a separate staging destination. Bound rows and
total bytes with explicit failures, never truncation. This first slice establishes
lossless preservation evidence, not Task 7 completion or permission to delete the
legacy databases/dependencies.

The first preservation slice is now implemented in
[`mdreader-legacy-export`](../../crates/mdreader-legacy-export/README.md): bounded
explicit offline selections, exact streaming raw copies, proposal v1/v2 and
known JSON interpretations, hashed manifests and private no-replace publication.
Synthetic tests cover all three proposal review states, missing-source/hash
divergence, unknown rows/virtual modules, exact large-integer/blob/real cells,
tamper, bounds, changed source identity and actual process death before publish.
The archive verifier and separate destination establish preservation evidence;
there is no reader-v1 importer, activation/rollback switch or live-profile
snapshot adapter. Windows publication remains fail-closed. These remaining
gates still block removal of legacy dependencies or a Task 7 completion claim.
