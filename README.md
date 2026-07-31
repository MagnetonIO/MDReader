# MDReader

A desktop Markdown library that indexes your notes — local folders, Google Drive,
Dropbox — and lets you search them semantically, with every answer able to say
where it came from.

MDReader is the reference application for [CatDB](https://github.com/MagnetonIO/CatDB),
and it consumes CatDB the way any other application would: as one versioned SDK
dependency, from outside the platform repository.

---

## What it is

A [Tauri 2](https://tauri.app) app — a Rust core with a React/CodeMirror
front end — that builds a searchable index over Markdown you already have.

- **Your files stay yours.** Sources are read-only. No adapter writes to a
  source, ever; the trait it implements has no write operation, so that is a
  property of the type system rather than a rule someone remembered to follow.
- **Cloud without lock-in.** Google Drive and Dropbox connect through OAuth
  PKCE. Credentials go to the OS keychain, never to a config file.
- **Answers carry provenance.** Search results name the source that supplied
  each field, and a value the app could not observe is *absent* — never a
  guessed default.

## Sources

| Source | Reads | Observes |
| --- | --- | --- |
| Filesystem | a directory tree | `path`, `size`, `modifiedAt` |
| Git | `rev-parse` / `ls-files` / `log` | `path`, `modifiedAt`, `author` |
| Frontmatter | the YAML block in each file | `path`, `title`, `author`, `body` |
| Google Drive | a Drive folder or file selection | Markdown documents + revisions |
| Dropbox | a Dropbox path | Markdown documents + revisions |

The first three read one local directory three different ways, which is why
they live here rather than in CatDB: they are document-library sources, not
enterprise data connectors.

## Layout

```
crates/
  mdreader-domain              CatDB schemas + mappings for the FS -> MD -> KN -> RX pipeline
  mdreader-ingest              scanner, parser/chunker, incremental ingestor; hosts the `catdb` binary
  mdreader-presentation        DTO shaping for the Tauri command layer
  mdreader-connector-local     filesystem / git / frontmatter sources
  mdreader-connector-google-drive
  mdreader-connector-dropbox
src-tauri/                     the desktop shell (commands, library, cloud accounts)
ui/                            React + CodeMirror front end
packages/                      shared TypeScript: config, markdown, ui
catdb/sdk/typescript/          `@catdb/client` — talks to a CatDB endpoint
fixtures/corpus/               a committed Markdown corpus the tests index
demo/                          curated corpus for the demo runtime
```

`mdreader-ingest` is the composition root: the platform boundary runs one way
(`catdb/*` never depends on `mdreader-*`), so the application side is where the
domain registers itself into the engine.

## Build

Requires the pinned Rust toolchain (`rust-toolchain.toml`, currently 1.94.0),
Node with pnpm, and the Tauri prerequisites for your platform.

```bash
pnpm install
pnpm --filter @mdreader/app tauri build
```

**Provider sign-in is compiled in.** The OAuth client IDs are read with
`option_env!`, so a build without them produces an app that reports
`client-configuration-missing` when you try to connect — correctly, but the
button will not work. Supply them first:

```bash
set -a; source .env; set +a     # cargo does not read .env
pnpm --filter @mdreader/app tauri build
```

`.env` holds `MDREADER_GOOGLE_DRIVE_CLIENT_ID`,
`MDREADER_GOOGLE_DRIVE_CLIENT_SECRET`, `MDREADER_DROPBOX_CLIENT_ID` and
`MDREADER_DROPBOX_REDIRECT_URI`. It is gitignored. The committed
`scripts/release/public-oauth-authority.json` carries SHA-256 hashes of the
public values, so a build can prove it used the right ones without the repo
ever holding them.

### macOS signing

Bundles must be signed with a real Developer ID
(`tauri.conf.json` → `bundle.macOS.signingIdentity`). Ad-hoc signing makes the
designated requirement the executable's *hash*, so every rebuild looks like a
brand-new app to macOS and every privacy grant you have granted is orphaned.

The `tauri` script puts `/usr/bin` ahead of pyenv shims on purpose — if `xattr`
resolves to a shim, signing fails silently and falls back to ad-hoc.

## Test

```bash
cargo test --workspace
pnpm --filter @mdreader/app test        # vitest
pnpm --filter @mdreader/app test:e2e    # Playwright, real browser engines
```

Browser e2e runs the real UI in real engines, which is the only place CSS and
selection are real — jsdom cannot see either. Know which mode a spec is in:
mocked IPC proves rendering, real-engine proves answers.

**Known failures: 15.** They are inherited, not new, and cluster on a single
invariant — *an ambiguous cloud write must stay `WriteOutcomeUnknown`*. Twelve
are in the Drive and Dropbox connectors, three in the shell. They are tracked;
a green run today means 15 failures, not 0.

## Relationship to CatDB

CatDB is the platform: the semantic layer, the query engine, the storage, the
federation. MDReader is an application built on it.

The dependency is a single `catdb-sdk` git dependency — the whole platform
behind one versioned facade, with connectors behind off-by-default features.
MDReader's own connectors implement CatDB's `DataSource` trait from *outside*
the platform repository, which is the check that the facade is actually
complete rather than merely present. (It has already earned its keep: moving
them out found two pieces of the surface that were private for exactly as long
as every connector happened to be a sibling crate.)

MDReader can also talk to a running CatDB deployment over HTTP through
`@catdb/client`, which is what the **Connected CatDB** section in Settings
configures.

## License

Apache-2.0 OR MIT.
