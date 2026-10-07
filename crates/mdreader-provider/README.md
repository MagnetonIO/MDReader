# MDReader provider contract (staged extraction)

This crate owns source/document DTOs, bounded I/O context, authenticated cursors,
provider errors, and a service-owned mutation journal. It has no CatDB, Tauri,
OAuth, keychain or database-file-opening dependency. Backend HTTP transport is
separate from reader DTOs; the optional `http-client` feature supplies a bounded,
redirect-disabled Reqwest implementation. No live provider access or private
provider writes were performed.

## Source/document boundary

`SourceBinding::new(library, source, account, generation, selection_digest)` binds
all operations to the exact admitted authority. IDs, record keys, cursors,
provider values, hashes and Markdown bytes have bounded constructors and custom
validated deserialization. Unknown DTO schemas fail closed. Debug output redacts
record identities, provider tokens, bodies, revision values and HTTP headers.

`SelectedScope` is explicit: exact documents, an explicit scoped root/recursion
choice, or an **imported** legacy broad configuration digest. No default expands
a source to the whole account or an inferred sibling folder. `SourceDescriptor`
validates the scope's digest against its binding and carries capabilities,
health and coverage. The service must issue a new generation whenever account,
selection, authorization profile, resource-key authority or source configuration
changes; a renderer-supplied descriptor is not an admission grant.

`DocumentVersion` separates:

- `provider_revision`: provider change/revision identity;
- `content_hash`: explicitly tagged SHA-256, MD5 or Dropbox content hash;
- `conditional_write_token`: exact provider conditional mutation validator.

A checksum never becomes an HTTP `If-Match` validator. Provider hashes are
verified against downloaded/accepted bytes. Reader/index chunk hashes remain a
separate local identity. Bodies are bounded UTF-8 Markdown (2 MiB).

`DocumentProvider` is object safe and supplies `descriptor`, `metadata`, `list`,
`read`, `start_changes`, `changes`, `resolve_asset` and `guarded_write`.
`ProviderAuthority` is supplied by the service and revalidates the immutable
binding before/after effects, including again after suspended credential
resolution. The callback, refresh and transport all share cancellation/deadline
bounds; no SQLite lock should be held while awaiting them.

Native enumeration returns `MetadataCompleteSelected`, never completed indexing.
Only the owner can publish `CompleteSelected` after bodies and index coverage are
actually established. Continuations cannot claim either complete state. Missing,
oversized, stale or failed bodies must remain `Partial`/`NeverIndexed` as
appropriate. Provider search bootstraps are not completeness evidence.

Change pages distinguish continuation from final durable checkpoint and explicit
rebuild/reset. `Change::Deleted { key, expected_path }` uses a locator guard for
path-only provider tombstones: the owner must ignore an old-path deletion when
that stable ID's published locator has since changed. Definitive stable-ID
removal/eligibility changes use `None`. Never advance a checkpoint before its
metadata/tombstones and jobs commit atomically in the service owner.

`CursorCodec` HMAC-authenticates provider, library, source, account, generation,
selection and cursor kind. The owner supplies a securely generated, locally
persisted zeroized key; it must not serialize/log that key or accept one from a
renderer. A cursor is authenticated, not encrypted, and remains private service
state. List and change cursors are not interchangeable.

## Durable guarded mutations

All functions accept the existing owner's `&mut rusqlite::Connection`. No second
connection or writer is introduced. The journal requires owner `synchronous`
FULL or EXTRA, preserves `user_version`/journal mode, and uses its own guarded
schema version (v2, with transactional v1 migration).

```text
journal::prepare(connection, binding, key, expected_version, intended_body)
  -> operation UUID after intent/body commit
journal::begin_dispatch(connection, binding, operation)
  -> DispatchPermit after prepared -> dispatched commit
provider.guarded_write(permit, body, context)
journal::accept_receipt(connection, binding, verified_receipt)
owner commits durable metadata/body admission and index job
journal::mark_published(connection, binding, operation)
```

The permit is neither Clone nor Deserialize. It binds the exact body hash,
record, expected version and complete source authority. A second dispatch is
never issued after a crash, timeout or uncertain response. Startup calls
`recover_interrupted`, inventories `pending_ids`, then performs read-only
reconciliation. `reconcile_read` retains both the intended body and a conflicting
observed body/version separately; a matching desired-body read establishes
observed convergence, not proof that this particular request authored it.
Unknown/conflict/accepted-but-unpublished records never auto-retry or disappear.
`accept_receipt` validates operation, identity, body hash and length before local
acceptance. An index-only success cannot acknowledge publication.

Limits: 128 unresolved intents, at most 64 retained published intents, 256 MiB
total body/record/conflict storage, 32 KiB intent/receipt metadata, 2 MiB bodies.
Storage pressure may prune published history, never unresolved/conflicting work.
`forget_terminal` is explicit owner-directed retirement of a published/conflict
record. Old pruned IDs cannot be dispatched. An exhausted conflict-storage bound
leaves the original intent unresolved and returns an error.

## Connector kernels and compatibility

Both connector crates export `native::Client` implementing `DocumentProvider`.
Their old APIs are preserved under the temporary default `legacy` feature.
`default-features = false` selects a dependency graph with no CatDB package.
Old source code is in `src/legacy.rs`, re-exported for the existing app; fixture
assertions were preserved, including the Dropbox source-inspection test's path.

Drive native supports **exact Picker-selected file IDs** under `drive.file`.
Resource-key headers remain bounded and header safe. File `version`, MD5 and
strong HTTP ETag remain distinct; body reads recheck revision/hash after transfer.
Missing strong ETag refuses guarded writes as `Unsupported`; no validator is
invented. Selected-file metadata listing and scoped change/reset are supported.
Native folder/My Drive/broad legacy grants and Google Docs exports are unsupported.
The provider owner must obtain an actual `drive.file` grant; the resolver trait
and scope constant do not implement an OAuth login or prove installed credentials.
[Google scopes](https://developers.google.com/workspace/drive/api/guides/api-specific-auth)
and [File resource](https://developers.google.com/workspace/drive/api/reference/rest/v3/files)
are the primary contracts checked on 2026-10-07.

Dropbox native supports exact selected stable file IDs and explicit app-folder
roots. Exact-file sources have `changes=false`; app-folder roots support bounded
`list_folder` enumeration/change/delete/reset. Source-authorized stable IDs are
learned from validated selected-root pages. `restore_membership(current_binding,
owner_persisted_selected_files)` restores bounded locator membership on restart;
fresh metadata/body scope still revalidates it. Folder tombstones retain path
fences and old aliases until owner reconciliation/checkpoint commit. Updates use
`mode:update(rev)`, `autorename:false`, `strict_conflict:true`; no blind overwrite
or second upload occurs after ambiguous response. Imported full-account modes
remain legacy. The owner resolver must enforce app-folder credential custody;
no new OAuth flow or permission expansion is performed. The pinned field and
operation contracts follow Dropbox's [official Stone specification](https://github.com/dropbox/dropbox-api-spec/blob/main/files.stone).

Credentials are resolved/refreshed only through provider-specific in-memory
interfaces. Grant tokens and cursor keys are zeroized; grants/request/response
Debug are redacted. One bounded refresh is shared across concurrent stale reads;
provider mutations are send-once and never retried for authentication failure.
Requests use fixed official origins, redirects are disabled, responses and
headers are bounded, and no arbitrary asset URL is fetched. Assets remain
explicitly unsupported.

## Registry handoff and remaining stage 3

The service registry should persist `SourceDescriptor` plus private provider
configuration/credential **references**, never raw tokens. Renderer list/status
responses should expose only selected source ID, name, kind, health, selected
scope description and honest coverage. Admit/reconnect/disable/forget/refresh
must be explicit; each authority change issues a new generation and cancels old
work. New grants start with `NeverIndexed`; no caller coverage assertion is
trusted. Imported legacy broad profiles require explicit separate admission,
not conversion to new narrow-grant acceptance.

The [service registry foundation](../mdreader-service/src/provider/README.md)
now implements staged snapshots, bounded metadata pages, checkpoint+changes
atomic publication, source epoch fencing, revision-bound cache, accepted
receipt/local-publication repair and bounded startup repair against the existing
owner connection. Its concrete core bridge admits provider-derived authority and
exact-revision jobs; it never treats remote cache as an ordinary local source.
The legacy `cloud_documents` app port and runtime scheduling remain separate.
Do not port its independent `Connection::open` or Tauri scheduling into this
library. Root/app wiring, source registry migration, private credential custody,
OAuth/Picker UX, live provider acceptance, assets, broad-profile replacement,
Linux/Windows runtime checks and complete searchable coverage remain incomplete.

## Verification

```sh
CARGO_TARGET_DIR=/Users/mlong/Library/Caches/MDReader/provider-target cargo test -p mdreader-provider -p mdreader-connector-google-drive -p mdreader-connector-dropbox --no-default-features
CARGO_TARGET_DIR=/Users/mlong/Library/Caches/MDReader/provider-target cargo test -p mdreader-connector-google-drive -p mdreader-connector-dropbox
CARGO_TARGET_DIR=/Users/mlong/Library/Caches/MDReader/provider-target cargo clippy -p mdreader-provider -p mdreader-connector-google-drive -p mdreader-connector-dropbox --no-default-features --all-targets -- -D warnings
cargo tree -p mdreader-connector-google-drive --no-default-features
cargo tree -p mdreader-connector-dropbox --no-default-features
```

All tests are synthetic transports or real temporary SQLite files. Record-key
validation, durable intent, separate conflict-body retention, hung authority
callback and changed Markdown eligibility were observed RED before fixes. This
is staged extraction, not Task 6 or release acceptance.
