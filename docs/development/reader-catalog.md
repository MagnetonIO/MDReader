# Reader-owned catalog boundary

The primary Library list and exact reader opens use the MDReader catalog.
Legacy metadata/body reads are available only through an explicit migration/history
action; startup and catalog browsing never hydrate the legacy Library.
The native Library catalog observes existing retained Library folders and
explicitly selected provider metadata. It never admits/regrants a folder, reads
credentials, enumerates remote metadata or fetches a body while listing. A folder
not yet admitted to the reader shows **Activate in Settings**; the existing trusted
native Library flow performs that explicit activation.

Each page has at most 100 documents and one lookahead row. The service retains at
most 8 opaque continuation handles for 30 minutes. A continuation carries the first
source-authority and index/publication snapshot, a private keyset position, and
page size. Reindex, revoke/regrant, reset, provider metadata publication or cache
replacement makes it stale. Successful page consumption replaces its cursor;
replaying a consumed cursor is explicitly stale rather than skipping a page.
Service restart also invalidates these volatile handles. Reload starts a new
observation, never silently continues an old page with a new snapshot.

Local observations reuse the existing read-only service connection. The durable
source nonce changes in the source writer's transaction; a completed derived
index gets one fresh snapshot in its existing transaction, including empty scans.
Namespace validation checks exact table/trigger definitions and version rows.
Historical index adoption preserves rows and existing grant state, but its scan
coverage/time remain unknown until a successful new scan. Provider catalog cache
and metadata epochs use triggers in the existing library writer transaction.
Provider sections fail busy instead of waiting for a long writer operation.

Rows expose actual indexed title, exact revision and modified time where observed.
Local byte sizes, author and Git provenance are not inferred. Provider selected
metadata coverage is distinct from admitted body coverage; no remote freshness is
claimed. Per-source failures remain visible; unavailable provider metadata does
not turn a valid local catalog into empty success. No public DTO includes folder
roots, cache roots, credential references or opaque provider record/version keys.

Exact opens carry canonical reader source ID, relative path and revision. Local
opens use the normal main reader and shared read-only rendering/navigation. They
recheck retained Library membership/root before and after the exact reader call. They do not construct an absolute path or
activate a workspace. Provider rows without a current admitted receipt offer only
an explicit **Fetch current** action using a service-minted document handle; its
returned canonical binding must match the expected source. Stale UI replies are
ignored, and unused cursors are released. Images remain unavailable in this
read-only catalog reader; existing explicit device asset support is separate.

Existing legacy document bodies, edit receipts, recents, history, exports and
rollback remain untouched. Their lossless preservation/mapping is still an
activation gate before removing the legacy SDK paths. Generic CatDB
schema/mapping/law/Enterprise administration is retired by the approved reader
plan; this feature does not introduce a replacement generic entity query API.
The retained legacy adapter is hidden behind **Legacy records (migration)** by
default. Existing historical Recents explicitly re-enter that adapter with their
original identity and edit receipt; they are never relabeled as canonical revision
evidence. New canonical selections invalidate pending legacy replies and appear
in the normal main reader. Settings source browsing retains a modal wrapper over
the same rendering/navigation implementation. Local relative/wiki navigation
wraps the existing source-scoped resolver with exact origin and retained membership
checks; ambiguity requires a choice. Canonical selections are currently volatile
read-only views; persistent canonical tabs/Recents and guarded catalog editing
remain follow-up work. This cutover does not claim full CatDB
removal or installed-profile migration acceptance.

Focused evidence is recorded in the [foundation receipt](reader-catalog-validation-2026-10-08.md)
and [primary cutover receipt](reader-catalog-primary-validation-2026-10-08.md).
No provider/network/model/installed-profile fixture is required for these tests.
