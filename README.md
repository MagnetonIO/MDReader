# MDReader

An offline-first Markdown reader for desktop, mobile and web. The reader-owned
SQLite core indexes local folders, agent outputs and selected Google Drive or
Dropbox documents. Search results retain source, revision and line evidence.
Local reading does not require a cloud account.

The reader migration is under development on `dev-reader-next`. Source changes,
combined-candidate verification, packaged upgrades and deployment acceptance are
separate milestones. See the [approved plan](docs/plans/sqlite-reader-next.md)
and [status ledger](docs/development/sqlite-reader-next-status.md) for remaining
release gates. Do not use historical validation receipts as proof of this tree.

## Start the desktop app

Install Node 24, pnpm as pinned in `package.json`, the pinned Rust toolchain and
[Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for your OS.
From this repository root:

```sh
pnpm install --frozen-lockfile
pnpm desktop:dev
```

This starts the native Tauri application and its frontend development server.
`pnpm dev` starts only the Vite frontend; it does not supply native commands.
Stop development with Ctrl-C. The launcher uses Node rather than POSIX shell
assignments, defaults Cargo to two jobs, and preserves the caller's environment.
It does not download models or log private configuration itself.

If `target` is an old cache symlink, its destination must exist. Restore that
cache directory or remove only the broken symlink. Do not delete a working Cargo
cache as routine troubleshooting. Fresh clones use Cargo's ordinary target
directory unless the developer deliberately configures another one.

The headless reader can run separately without model loading:

```sh
cargo run --locked -p mdreader-service -- --no-model /ABSOLUTE/PRIVATE/READER-DATA
```

Choose a new private development directory, not an installed legacy profile.
The service owns SQLite and source grants. It does not infer access to your home
directory or agent histories. Desktop startup owns its own service configuration;
this explicit command is useful for headless/MCP development.

## Storage and source access

`library.sqlite` holds durable library state; `index.sqlite` holds rebuildable
search data. Original local/provider Markdown and durable captured outputs stay
authoritative. Source reads and search leave Markdown unchanged; the editor
writes only on an explicit guarded save. Cloud credentials remain in private
platform custody. Optional encrypted sync transfers selected records and remains
a development preview pending release acceptance.

Keep backups of Markdown and durable state. Rebuilding a search index is not a
library backup. Legacy CatDB data requires explicit offline export/migration;
retain the original profile and use a separate destination for rollback. Do not
point the new reader at a legacy database and assume it will upgrade in place.

## Source layout

| Directory | Responsibility |
| --- | --- |
| `crates/mdreader-core` | SQLite library, indexing, search and exact evidence |
| `crates/mdreader-service` | Headless owner, scheduling and bounded IPC |
| `crates/mdreader-provider`, `mdreader-connector-*` | Selected Drive/Dropbox access and guarded operations |
| `crates/mdreader-capture`, `mdreader-mcp`, `mdreader-integrations` | Agent output capture and read-only client retrieval |
| `crates/mdreader-sync` | Encrypted client protocol and durable sync state |
| `crates/mdreader-legacy-export` | Offline legacy preservation and export |
| `src-tauri` | Native composition and platform adapters |
| `ui/src`, `packages` | App features and shared Markdown/reader presentation |
| `extensions/vscode` | Local desktop VS Code extension |
| `scripts`, `docs` | Development, packaging and maintained contracts |

The companion [MDReader-Web](https://github.com/MagnetonIO/MDReader-Web) repository
owns the Next.js website, browser SQLite reader and Cloudflare sync server. Run
its `pnpm dev` from that checkout for `/app`; it is not hosted by this Vite app.

## Providers and packaging

Local reading needs no OAuth configuration. Google Drive/Dropbox sign-in needs
the configured provider application identifiers and exact callbacks. The native
build reads compile-time configuration; exporting variables after compilation
does not change an existing app. Keep private `.env` configuration ignored and
never print it. The release helper checks configured public identifiers against
`scripts/release/public-oauth-authority.json`.

```sh
pnpm tauri build
```

That builds a native package, not a verified release. On macOS, release builds
need the configured Developer ID signing identity and notarization; the launcher
puts system tools ahead of Python shims for signing. Windows/Linux and mobile
packages have their own host, signing and lifecycle acceptance requirements.
See `scripts/release/` and the platform workflows for reproducible delivery.

## Verification workflow

Builders implement and integrate usable source slices without per-feature
review/testing cycles. One combined integration candidate receives independent
review, appropriate tests, browser/native acceptance and bug fixes before `dev`.
Use affected suites at that stage and keep one heavyweight command across the
team. Do not default to `cargo test --workspace` for a local change.

Relevant commands at that checkpoint include:

```sh
cargo test --locked -p mdreader-core
pnpm --filter @mdreader/app typecheck
pnpm --filter @mdreader/app test
```

Select additional service/provider/sync/client/platform checks for the actual
candidate. Unit results do not establish provider OAuth, installed-client trust,
physical mobile behavior, a deployed sync roundtrip or a signed upgrade.

## License

Apache-2.0 OR MIT.
