# Profile SQLite identity and lock preservation

Root owns `fix/candidate-sqlite-locks` in the isolated
`MDReader-candidate-sqlite-locks` worktree, based on integration `4aa9037`.
Destination is `dev-reader-next`. No original/installed profile is migrated by
this change. All databases used for verification are disposable synthetic inputs.

## Observed defect and correction

Candidate verification and snapshot source verification opened a second file
descriptor on the live SQLite pathname and closed it. Inventory also opened and
reopened every SQLite/recovery file. On the tested macOS host those closes released
the process's SQLite locks: a competing process obtained an incompatible writer
lock while the original connection still believed its transaction was protected.
The old identity check compared two fresh opens of the same pathname, so it also
accepted a retained connection after its named file had been replaced.

This matches SQLite's documented [POSIX close/lock interaction](https://www.sqlite.org/howtocorrupt.html#posix_advisory_locks_canceled_by_a_separate_thread_doing_close_).

The profile crate now validates the actual borrowed main connection using
`SQLITE_FCNTL_HAS_MOVED`, bracketed by complete nofollow file stamps under retained
root/parent directory identities. Directory-only `statat` traversal rejects
symlinks, nonregular files and multiple hard links without opening the database
inode. Candidate verification additionally requires the exact prepared root
binding and `library.sqlite`, and rechecks after receipt writes/reads.

Inventory retains file descriptors only for explicitly classified raw artifacts;
SQLite and excluded recovery/lock members use metadata only. Snapshot backup still
uses SQLite's backup API on the actual owner's borrowed connection and revalidates
that handle both before and after copying. Archive files are hashed only after the
archive SQLite connection has closed. No backup format or migration schema changed.

## Evidence

- Two candidate regressions exposed the original lock-loss / false-identity bugs.
- Three added live-database cases failed against the original inventory/backup
  helpers: inventory capture/revalidation/drop lost the owner lock, snapshot
  validation lost the owner lock, and replacement falsely attested an old handle.
  Restored implementation passes all three, using actual child SQLite processes.
- An end-to-end completed-snapshot case verifies that staging and subsequent
  archive verification preserve the real owner's rollback-journal read lock.
- Final affected crate suite: **12 pass**, 0 fail, 0 ignored; warm compilation
  1.41 s and test execution 0.23 s on macOS.
- Strict all-target profile Clippy: pass, 0.62 s. Formatting of touched files and
  diff-check pass. Jobs limited to two, shared warm target; no model/full-app/
  packaging build. No manifest, lockfile, dependency, UI, RPC or deployment edits.
- `6fda276` integration changes neither this crate nor its legacy-export dependency,
  workspace manifest or lockfile relative to the tested `4aa9037` base.

## Boundaries and remaining acceptance

A reviewed native owner must classify its actual SQLite and recovery paths
correctly: labelling a live DB/SHM file `Raw` is not safe. The generic Inventory
API cannot infer that classification without opening the inode; production owner
registration/classification remains part of native migration integration.
Unsupported VFS/hosts refuse identity proof. This host does not establish Linux,
Windows or packaged upgrade acceptance. Full all-participant native freeze,
service transport/backup composition, useful-record conversion, installed-library
migration/export/rollback and release packaging remain open gates.
