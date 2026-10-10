# MDReader repository rules

## Current delivery workflow

Use two implementation builders, frontend and backend, plus an independent
reviewer. Deliver usable, scoped source commits into the integration branch.
Do not run a review, test, mutation or validation-document cycle for every
feature; compile only when needed to unblock implementation. Run one coherent
combined-candidate review, testing and bug-fix stage before merging into `dev`.
This delivery timing overrides older per-feature timing below; data-loss,
authority, privacy and source-ownership requirements still apply throughout.

These rules apply to agents and human contributions in this repository.
The approved product contract is [the SQLite plan](docs/plans/sqlite-reader-next.md).
Use [the status ledger](docs/development/sqlite-reader-next-status.md) to distinguish
implemented work, runtime evidence and unfinished release gates.

## Architecture and module ownership

- **No God files or God objects.** Each production module owns one cohesive
  responsibility. Separate domain rules, persistence, transport, platform
  integration and presentation. Composition roots wire dependencies; they do
  not implement business rules, SQL, OAuth or rendering behavior.
- Keep new production modules around 400 lines or fewer as a review trigger,
  not a target to game. Split mixed responsibilities before adding features.
  Large fixtures/generated files have different constraints. Existing oversized
  modules are debt: extract the touched responsibility instead of adding another
  subsystem or performing an unrelated wholesale rewrite.
- Prefer feature directories with small named modules, private implementation
  details and an explicit public interface. Avoid catch-all `utils`, global
  mutable registries, duplicate abstractions and circular dependencies.
- Core contracts belong to MDReader. Tauri, OAuth and Cloudflare stay behind
  their adapters. No CatDB engine, SDK, client, connector or runtime domain types
  belong in MDReader, including optional dependencies. Historical filenames and
  wire discriminators may exist only in offline import/preservation boundaries
  and their fixtures. Retired records are opaque archives, never live source
  membership, credentials, grants, capabilities or index readiness.
- SQLite remains the chosen database. Original Markdown/provider bodies and
  durable captures are authoritative; `index.sqlite` is rebuildable. One native
  owner controls writes/model state. Never add permanent dual writes.

## Directory structure

| Location | Responsibility |
| --- | --- |
| `crates/mdreader-core/` | Portable source/document/evidence/indexing rules and SQLite ownership |
| `crates/mdreader-service/` | Headless owner, bounded IPC, scheduling and adapter composition |
| `crates/mdreader-provider/`, `crates/mdreader-credentials/` | Provider contracts and private credential custody |
| `crates/mdreader-capture/`, `crates/mdreader-mcp/`, `crates/mdreader-integrations/` | Explicit agent capture, retrieval and client configuration |
| `crates/mdreader-sync/` | Versioned encrypted client protocol and durable sync state |
| `crates/mdreader-legacy-export/` | Explicit offline preservation and archival migration |
| `src-tauri/` | Thin native commands, platform adapters and app startup |
| `ui/src/` | App features, platform adapters and client composition |
| `packages/` | Shared Markdown, reader UI and configuration packages |
| `extensions/vscode/` | Local extension-host adapter |
| `fixtures/`, `demo/` | Small public or synthetic fixtures |
| `docs/`, `scripts/` | Maintained architecture/setup/evidence and reproducible tooling |

The companion MDReader-Web repository owns the Next.js site, browser SQLite
worker and Cloudflare server. Its `AGENTS.md` governs changes there. Existing
legacy domain/ingest/admin modules are migration inputs, not new extension points.

## Rust practices and dependency upgrades

- Use the pinned stable toolchain in `rust-toolchain.toml`. Prefer current
  maintained stable libraries/frameworks compatible with supported targets.
  Review official release/migration notes before upgrades. Update compiler,
  edition, framework and dependency versions deliberately in focused changes;
  never run a blanket upgrade during an unrelated fix.
- Declare/test the supported Rust version when establishing an MSRV policy;
  a compiler pin alone does not prove the oldest supported compiler. Check
  desktop/mobile target support and feature combinations before raising it.
- Commit `Cargo.lock` and `pnpm-lock.yaml`; use locked/frozen installs and
  verification. Inspect dependency/version/checksum drift. No new dependencies
  on developer-specific absolute paths or local `file://` repositories.
- Keep optional platform/model/provider features narrow. Use workspace
  dependencies for genuinely shared versions. Add a crate only for a real
  ownership/reuse boundary; do not add a framework for a small helper.
- Prefer typed enums/newtypes and `Result` with useful domain errors. Do not
  panic, `unwrap` or silently default on recoverable production failures.
  Use narrow visibility, documented public contracts and RAII cleanup.
- Keep `unsafe_code` denied. Avoid blocking work on async/UI executors; use
  bounded blocking workers. Never hold a database transaction or owner mutex
  across network/OS waits or expensive image/model work. Revalidate authority
  after those waits, and bound time, bytes, queues and concurrency.
- Use parameterized SQL. Preserve exact revisions and distinguish absent,
  stale, unsupported and unknown outcomes. Secrets/private paths must not leak
  through logs, errors or derived `Debug`/serialization implementations.
- Run formatting and strict Clippy for the affected crate at a stable
  checkpoint. Do not suppress a lint globally to hide a local design problem.

## Persistence and migrations

- The owning crate owns versioned migrations in a dedicated module/directory;
  split growing schema SQL into ordered migration files. Use the existing
  rusqlite architecture. Do not introduce an ORM or migration framework solely
  to replace a working small migration runner.
- Applied migration history is immutable. Add the next migration; check schema
  versions and reject unsupported newer durable schemas without resetting data.
  Apply each supported schema transition and its version marker transactionally.
- Separate durable-library upgrades from disposable-index rebuilds. Back up or
  export durable records before destructive transitions. Prefer additive,
  backward-compatible changes; document old-client and rollback constraints.
- Test relevant prior-schema fixtures, reopen, interrupted/replayed migration
  and preservation of originals/user state. A fresh empty database test alone
  does not establish upgrade safety. Never drop durable data to make a test pass.
- Migration from CatDB requires lossless export, verified useful-record mapping,
  explicit source binding and separate-profile rollback before removal.

## Focused testing and resource discipline

Follow [testing-policy.md](docs/development/testing-policy.md).

- Start with the smallest meaningful regression. Iterate on it, then run the
  affected module/crate suite. Broaden for changed shared contracts, failures or
  unresolved integration risk. Do not run thousands of tests for a local fix.
- Run **one heavyweight build/check at a time across agents**. The coordinating
  agent assigns the slot. Cargo defaults to two jobs; use `CARGO_BUILD_JOBS=2`
  outside this tree. Keep Android Gradle at two workers with parallel projects
  disabled. These settings limit concurrency, not CPU percentage.
- Reuse compatible caches. No routine `cargo clean`, cold targets or simultaneous
  desktop/iOS/Android packaging. Finish a coherent slice before packaging it.
- Retire unused isolated build targets and superseded acceptance app bundles at
  delivery checkpoints. Keep one shared warm target, the running candidate and
  one rollback build. Check target symlinks, agent ownership, running processes
  and open files before removal. Preserve receipts, logs, private configuration
  and library/migration data; disposable build output is not a library backup.
  Record reclaimed space and avoid recreating per-feature target directories.
  MDReader builds must use MDReader-owned cache directories. On this Mac the
  shared native target is `~/Library/Caches/MDReader/cargo-target-native`; use
  `CARGO_TARGET_DIR` or an inspected `target` symlink pointing there. Never use
  another project's build cache. Preserve compatible MDReader artifacts when
  relocating a target rather than forcing a cold rebuild.
- Use real SQLite for durability/reopen/grants; real subprocesses for process
  boundaries; a small host/browser set for lifecycle behavior. Mock only the
  boundary being isolated, not the storage guarantees being claimed.
- Test synthetic/public inputs in disposable profiles. Never index private
  agent histories or use installed libraries/provider originals as fixtures.
- Keep CPU/RSS/size probes separate from ordinary tests. Do not repeat model
  10k/50k benchmarks as routine feature verification. Build size is not memory
  usage; a compiler spike is not reader idle CPU.
- Generate Rust→TypeScript bindings from their source; do not hand-edit them.
  `scripts/check-bindings.sh` is the focused two-generator drift gate. Full
  baseline/cross-host/release checks run at integration or release checkpoints.

## Fast local startup

Run from the selected development worktree. Use Node/pnpm versions declared in
`package.json`, the pinned Rust toolchain and the official Tauri host prerequisites.

```sh
pnpm install --frozen-lockfile
pnpm tauri dev
```

`pnpm dev` starts only the Vite frontend; it does not start the native app.
Before a first Cargo build, inspect `readlink target` if `target` is a symlink
and verify that its cache directory exists. Never replace/remove a target link
without checking what owns its destination.

For an explicit disposable library with no model download:

```sh
CARGO_BUILD_JOBS=2 cargo run --locked -p mdreader-service -- --no-model /ABS/PRIVATE/DISPOSABLE_LIBRARY
```

Choose/create that private directory explicitly. Providers and sync are optional
and must report unconfigured status honestly; local reading must not require
cloud credentials. `pnpm tauri dev` loads only allowlisted native build configuration from the
main checkout's ignored `.env`/`.env.local`, then this worktree's overrides.
Explicit shell/CI values win. Existing Google/Dropbox client ID and Dropbox
redirect names remain supported; this reuses registrations, never old grants.
Google Desktop token parameters also reuse the native-only ignored configuration;
they never enter Vite-prefixed values, renderer DTOs or serialized grant identity.
See `.env.example`; keep actual secrets/tokens out of Git and renderer configuration.

Setup documentation must provide prerequisites, exact root-relative commands,
expected readiness, stopping/cleanup and common failures. Keep it updated when
scripts/configuration change. Verify a fresh-clone startup at release checkpoints.
The shipping reader no longer depends on the machine-local legacy SDK. Fresh-clone
readiness still requires an actual locked install and startup at the release checkpoint.

## Parallel worktrees, integration and cleanup

- `dev` is the development destination. Major feature branches may be retained
  while the feature evolves. `dev-reader-next` is the retained reader migration
  integration branch; it is not a shared editing checkout.
- Give each parallel agent or independent feature/bug one named branch and its
  own worktree, based on the agreed integration commit. Never have agents edit
  the same worktree, share a Git index, or copy unfinished source between trees.
  The coordinator owns the integration worktree; implementation also belongs in
  a separate feature worktree. Record owner, branch, path, base and dependencies.
- Commit only scoped, reviewed changes. Minor feature/bug branches merge into
  their owning major feature branch after focused checks and review. A reviewed
  major feature branch then merges directly into `dev`, or into an integration
  branch that merges into `dev`. Choose the route up front and resolve dependent
  changes in order. Run appropriate combined-tree checks at each merge boundary;
  do not merge unvalidated work into `dev`. Deployment acceptance remains a
  separate gate; a merge alone does not establish a successful release.
- Keep the existing single heavyweight build/check slot across all worktrees.
  Reuse compatible caches with serialized writers; do not start a cold build or
  install another dependency tree merely because an agent got a new worktree.
- After a minor feature/bug is accepted and its commits are reachable from its
  major feature or integration destination, remove its clean, inactive worktree
  with `git worktree remove`, then delete its merged local branch with
  `git branch -d`. Keep major feature branches intentionally while work continues;
  clean up their unused worktrees independently. Retire an integration branch and
  its worktree after it reaches `dev` only when no further integration is planned.
  Inspect status, untracked files and running processes first. Preserve private
  configuration and acceptance receipts before cleanup; never use forced removal
  or discard unmerged work to make cleanup pass. Prune stale Git worktree metadata
  after removal. Do not delete unrelated historical branches or worktrees.

## Clean repository and delivery

- Work in isolated development branches/worktrees. Preserve unrelated changes,
  installed baselines and private profiles. Stage only owned, reviewed paths.
- Keep the root minimal. Commit source, lockfiles, small deterministic fixtures,
  required generated contracts and useful docs. Ignore build/cache output,
  binaries, credentials, databases, logs and scratch artifacts. Keep large probe
  artifacts outside Git; commit sanitized bounded receipts when useful.
- Remove obsolete imports/code/fixtures within the changed scope. Do not keep
  parallel implementations or commented-out experiments. Retained legacy code
  must have an explicit migration reason and removal gate.
- Before handoff inspect status/diff, formatting and appropriate checks. State
  what changed, what actually ran and what remains unverified. A clean commit
  does not authorize discarding another contributor's unfinished work.
- Keep deployment/resource changes reproducible in the companion Wrangler
  configuration. Use its local-first tests and documented migration rules.
  Provisioning infrastructure is distinct from deploying/enabling sync.
- Vercel website delivery follows the companion web repository's `AGENTS.md`:
  verify the exact project/root, keep Preview and Production resources/secrets
  separate, use locked package builds and test the actual deployment URL.
  Preserve the Vercel website / Cloudflare sync boundary. Local/browser SQLite
  remains device-owned; do not move it into ephemeral Vercel Function storage.
  Native packaging, web preview, production promotion and sync activation each
  require their own evidence; share one heavy-build slot across all of them.

Primary references: [Rust MSRV policy](https://doc.rust-lang.org/cargo/reference/rust-version.html),
[Cargo CI](https://doc.rust-lang.org/cargo/guide/continuous-integration.html),
[Cloudflare local development](https://developers.cloudflare.com/workers/local-development/).
