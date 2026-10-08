# Native profile storage classification

Root owns `fix/profile-storage-layout`, based on integration `811ceaf`, with
`dev-reader-next` as its destination. Verification uses disposable synthetic
profiles and actual SQLite connections, never installed libraries.

## Correction

Native participant registration now distinguishes durable SQLite databases from
rebuildable indexes. The registration reserves each database's main path and its
WAL, SHM and rollback-journal paths, rejecting overlapping declarations.

Before freezing any participant or opening an inventory, both preservation entry
points validate selections against those declarations. Durable main files must
use owner-connection SQLite backup and cannot be omitted; rebuildable indexes and
known recovery files must be excluded from raw copying. Unknown SQLite/index/
recovery classifications and duplicate selections are refused. Only durable
registrations can provide a backup connection.

## Evidence

- The original implementation failed the actual native regression: a registered
  live SQLite main file labelled `Raw` was accepted for archival.
- Restored implementation passes both new regression cases. They cover durable
  main/WAL/SHM raw-copy refusal, omitted durable files, rebuildable main/WAL/SHM
  refusal, unknown database/recovery declarations, and accepted metadata-only
  index exclusion while the actual index connection remains usable.
- Rejection occurs before participant flush, preserves existing settings bytes,
  and publishes no archive.
- Affected actual-app `profile_transition::` suite: **11 pass**, zero failures,
  execution 0.24 s; warm compilation 4.56 s. Focused two-case run also passes.
- Formatting and `git diff --check` pass. Cargo uses the existing warm target,
  locked/offline resolution and two jobs. No dependencies, DTOs, model downloads,
  full-suite tests, packaging or deployment changes.
- Two inherited native test dead-code warnings remain. Full-native strict Clippy
  has existing broader debt and is not claimed green by these tests.

## Remaining boundaries

This enforces declarations; it cannot infer an undeclared database disguised as
an ordinary raw file. Actual native owners must declare every live database and
index. Startup still lacks the complete real writer/custody composition, so this
does not enable installed preservation, conversion, activation or rollback.
Cross-process backup, installed-library and cross-host release acceptance remain
separate gates.
