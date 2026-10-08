# Profile preservation and lifecycle owner

Portable, CatDB-free profile inventory/staging and durable selection transactions.
The five focused synthetic fixtures and strict crate Clippy pass. This code is
not registered in native startup or accepted against an installed profile.
No installed data was opened or changed.
The native `profile_transition` adapter deliberately returns `NotQuiescent`:
all eight actual writer participants still need registration/integration.

`Inventory::capture(explicit_root, explicit_classifications, limits)` retains the
root/file identities and directory entry stamps, refuses symlinks/special files,
accounts for every file/empty directory and rejects missing/duplicate selection.
Profile-owned root/relative path locators preserve native units, including an
existing protected `reader-v1` root; the legacy exporter's restrictions remain.
Unclassified entries can be reviewed but cannot produce a complete snapshot.
The known `reader-endpoint.json` bearer artifact can only be excluded as runtime
state. Unknown opaque bytes still require classification; no secret scan or
credential/vault enumeration occurs. Replies/native UI must sanitize locators.

`stage(&mut FreezeOwner, inventory, explicit_destination, limits)` is the only
snapshot producer. Its scoped callback requires a real native writer freeze and
strict JSON success acknowledgement. `FrozenProfile::with_database` yields the
actual owner connection; the crate checks its file identity and uses SQLite's
online backup API, including WAL, rather than copying an active database file.
Backup destinations use standalone DELETE journal mode so later reads cannot
create WAL/SHM members outside the verified archive.
For a legacy owner without a borrow API, only its real shutdown/drain capability
may yield a read-only backup connection after closing every writer and denying
restart, retaining/checking original database and sidecar identities. No SDK
reopen/migration or caller-supplied offline assertion substitutes for that proof.
The callback must yield an exclusive snapshot handle outside unrelated owner
mutexes. Raw non-SQLite files are streamed and hashed; all source stamps/entry
sets and the freeze are revalidated before private no-replace publication.
Busy/locked backup refuses quiescence; steps, pages, file/aggregate bytes and
manifest depth/counts are bounded. Unix implementation only; unsupported hosts
fail closed. Incomplete private staging directories are retained without a
completed journal receipt; no automatic cleanup or restore exists.

`verify(archive, expected_identity, limits)` checks manifest SHA-256, exact member
set, private files, identity/stamps, lengths/hashes and classifications. Hashes
detect corruption against an explicitly expected identity; they do not
authenticate an attacker-controlled manifest and identity together. A verified
archive does not grant a source or activate a profile. The existing offline
legacy export interpretation can consume selected standalone snapshot members
later; its `reader-v1` exclusion and offline contract are unchanged.

`candidate::prepare` verifies preservation outside the destination owner lock.
`candidate::record(&Transaction, plan, imported_records, import_digest)` publishes
only the immutable import receipt after actual domain rows on the existing
candidate owner transaction. It does not import domain rows, create grants,
invent review history or commit. A rollback drops both rows/receipt/schema.
`candidate::verify_committed` requires autocommit and exact actual candidate
connection/root binding; a receipt visible in an uncommitted transaction cannot
establish readiness. Native owner also verifies the converted records described
by `import_digest`; receipt storage alone is insufficient semantic conversion.

`lifecycle::Journal::attach(&mut Connection)` borrows the sole native selection
owner's connection and opens no SQLite file. Its ordered namespace migration
leaves unrelated tables/global schema versions untouched and rejects newer or
malformed layouts. `bootstrap` takes an actual captured inventory for explicit
initial setup; it never overwrites an existing different selector.
`prepare/preserved/staged` record an opaque operation, exact original/inventory,
expected snapshot and actual committed candidate receipt. `recover` is bounded
and supports restart without inferring profile/path/source adoption.

`activate/rollback` require a real `ActivationOwner` freeze implementation and
exact generation CAS. Activation selects only a candidate already committed on
its owner. Selector root, explicit legacy/reader-only runtime mode, state and
immutable activation receipt commit together on the lifecycle connection, with
final owner/candidate checks after every SQL
write. Candidate import and selection are two ordered commits, not a cross-file
atomic transaction. Lost receipt replay returns historical evidence and never
reselects; use `selection()` for the current profile. Generation exhaustion
refuses switches without wrapping. No record is silently pruned (1024-operation
bound); an explicit retention/export policy is a future gate.

Rollback always requires owner-derived fresh preservation/reconciliation of
post-activation writes and remote unknown outcomes. It changes only selection;
original/candidate files, user state and vault entries remain untouched. Startup
must route before opening either legacy/reader owner. Current native startup
opens both and has no selector routing, so production activation is unavailable.

Five authored private-seam fixtures cover WAL/unknown/raw preservation and
tamper, changed/unclassified/undrained refusal, committed candidate/selector
reopen/replay/rollback, final fence/late SQL rollback and future namespace/
interrupted candidate preservation. Private fake freeze
participants prove orchestration/storage only; they do not prove real renderer,
legacy cloud, detached daemon, hook producer or installed-profile quiescence.
See [the transition design](../../docs/development/installed-profile-transition.md)
for precise runtime hooks, source inventory and remaining staged implementation.

Observed foundation gate (2026-10-08, base `38b413d`, warm cache, two Cargo jobs):

```sh
cargo test --locked --offline -p mdreader-profile --lib
cargo clippy --locked --offline -p mdreader-profile --all-targets -- -D warnings
```

Five tests passed in 0.13s; final compile 0.48s. Strict Clippy passed in 1.58s.
Removing only the final selector root/mode/generation check made the existing
late-trigger regression fail; the guard was restored before the final passing
suite. Initial failures corrected synthetic temp canonical/private paths,
manifest-relative locator use, publication rename identity comparison and backup
WAL-mode inheritance. No registry version/checksum changed; lock resolution added
only this workspace package. These checks establish portable owner-seam behavior,
not real writer registration, native app compilation or installed cutover.
