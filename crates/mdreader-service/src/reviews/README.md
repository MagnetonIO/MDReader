# Explicit archival review observation

The desktop owner exposes `Request::Reviews { request }` for an explicitly
selected, verified `mdreader-legacy-export` review stage. This is read-only
archival interpretation and current-text comparison, not profile migration or
review promotion. There is no installed-profile discovery, generic SQL/admin
access, inferred source binding, grant creation, model use or provider network
fetch.

`Select { stage, identity: { exportId, manifestSha256 } }` verifies the completed
stage and replaces the service's one volatile selection. Other operations use
its random opaque handle: `List { handle, offset, limit }` (1–100 rows),
`Get { handle, recordId }`, `Compare { handle, recordId, sourceId, path, revision }`
and `Close { handle }`. Restart requires explicit reselection. A versioned,
namespaced library table stores only a stable profile UUID. Unsupported schema
or platform initialization disables reviews without breaking local reading.
Windows remains unsupported by the underlying private archival stage boundary.

One operation may hash/read the archive at a time; contention returns `busy`.
Hashing occurs outside the SQLite owner mutex. Retained profile/stage identities
and namespace version/UUID are checked before and after reads. Comparison borrows
the existing selected owner for two exact-revision document reads, with enabled
source, retained-root and provider-generation/cache checks intact. It neither
opens a second writer nor grants a path from an archival locator. Provider reads
are admitted cached observations; remote head freshness remains unknown.

Replies omit original archive locators and preserve recorded rulings, reviewers,
row identity/hash, rejected tombstones and explicit unknown interpretations.
`recordedCurrent` and `reviewedCurrent` are distinct normalized inclusive-span
hash comparisons. `currentPassage` contains the actual compared lines at the
returned exact revision, not a jump to a later file. Equality establishes cited
text equality only. Graph membership and chronology remain unavailable. The
original archived row/export remains authoritative and is never mutated.

Replies are bounded to 4 MiB, pages to 100 rows, current UTF-8 bodies to the
existing 2 MiB owner limit, and archive opening to the exporter's hard ceilings.
Errors are fixed typed categories without private paths. The desktop command
uses a native folder picker; renderer input includes only the explicit export
identity. Local comparisons additionally fence the captured workspace generation
through the final response, including same-root reopen. The reader panel begins
with no selection and displays compared text as escaped plain text.

Focused verification commands (repository root, one build slot, two jobs):

```sh
CARGO_BUILD_JOBS=2 cargo test --locked --offline -p mdreader-service --lib reviews::tests
CARGO_BUILD_JOBS=2 cargo test --locked --offline -p mdreader-service --test archived_reviews_process
CARGO_BUILD_JOBS=2 cargo clippy --locked --offline -p mdreader-service --all-targets -- -D warnings
```

Tests use only small synthetic archives and isolated temporary profiles. They
cover preserved unknown/rejected rows, local and provider exact comparisons,
revocation/edit races, replaced root/profile, restart/reselection, future schema,
concurrent operation rejection and the real service process boundary. Removing
final current-document revalidation makes the revoke-race regression fail.
Native/UI verification and actual packaged desktop acceptance are recorded
separately; passing service tests does not establish installed-profile export,
activation, rollback or complete CatDB removal.
