# MDReader

**Read, edit, and find your Markdown in one place.**

MDReader is a desktop Markdown library for your own folders and selected Google
Drive and Dropbox connections. Open a document, follow its links, adjust the
reading appearance, and return to your saved place. Local reading works without
a cloud account.

## Get MDReader

[Downloads](https://github.com/MagnetonIO/MDReader/releases) ·
[Installer plan](docs/releases/desktop-preview.md) ·
[Build from source](#build-from-source)

Desktop installers for **macOS, Windows, and Linux are planned**. No public
installer release is available at this checkpoint. macOS is the primary preview
validation platform; Windows and Linux packages still need their release checks.
Use the installer plan and release notes for the availability and limitations
of each published package.

Android and iOS are under development, with selected emulator/simulator checks.
They are not shipping mobile releases. A browser reader is also under development
in a separate project.

## Your Markdown library

- **Read comfortably.** Choose type and size, use the outline, follow Markdown
  links and footnotes, and keep your reading place. View supported linked images
  and text attachments within the selected source.
- **Edit deliberately.** Edit local files and write-enabled cloud documents, save
  changes, preview the unsaved draft, or keep source and live preview together in
  Split. Save keeps your editing view open. Create named Markdown documents
  in supported connected cloud folders.
- **Find what you need.** Jump to a filename or search indexed document content.
  Optional local semantic search has a separate model download and readiness
  state.
- **Choose your folders.** Added folders are indexed and monitored while the app
  runs, even when another folder is open. Pause updates per folder. Home or disk
  selection is an explicit opt-in.
- **Connect selected cloud sources.** Browse Google Drive and Dropbox Markdown,
  keep cached lists visible during discovery, and choose folder, subfolder, and
  editing options when connecting. Metadata discovery and searchable document
  content are separate.

MDReader keeps its library and search index locally in SQLite. Your Markdown
originals remain in their source folders or providers. Keep backups of those
originals; a search index is not a backup. Optional encrypted sync remains a
preview with separate release gates.

## Edit and preview

Use **Edit**, **Preview**, or **Split**, then **Save** when ready. Preview uses your
current draft; it does not save the file. Narrow windows offer Edit and Preview.
The optional editor menu has formatting commands and **Save a copy…**.

Use **Cmd** on macOS or **Ctrl** on Windows/Linux with **S** to save, **F** for
find/replace, and **+ / −** to adjust document text size. Standard undo/redo and
bold, italic, link and list shortcuts work in the source editor. Reading and
editing share text size; toolbar size stays unchanged.

Unsaved desktop drafts are checkpointed privately for recovery. Restoring a draft
requires reopening its source document; saving to an original or cloud provider
remains an explicit action.

## Work with agents

In **Settings → Agents & integrations**, choose **Codex** or **Claude Code** and
press **Install integration**. Select the sources the agent may read, then
explicitly **Enable MCP**. Restart the agent and follow its normal trust prompts.
MCP provides read-only retrieval; installing an integration does not share all
your files or enable response capture.

Markdown an agent writes in a chosen folder uses normal folder indexing. Optional
final-response capture is separate and off by default. See the
[agent setup guide](docs/development/agent-setup.md).

A portable public skill installer from this repository is being prepared. Its
command will be advertised after the repository and skill asset are published
and verified. Direct installation inside MDReader is available in the desktop
preview.

## Build from source

Install **Node 24**, **pnpm 11.16.0**, the Rust toolchain pinned in
`rust-toolchain.toml`, and the
[Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for your OS.
From the repository root:

```sh
pnpm install --frozen-lockfile
pnpm desktop:dev
```

This starts the native app and its frontend. Stop it with Ctrl-C. `pnpm dev`
starts only the frontend, without native commands. To build a desktop package:

```sh
pnpm tauri build
```

A locally built package is not a verified release. Provider sign-in requires the
native provider configuration described in `.env.example`; keep credentials out
of Git. Local reading needs no OAuth configuration. Signing, notarization, and
platform acceptance are described in the
[installer plan](docs/releases/desktop-preview.md).

For architecture, development boundaries, and verification commands, see
[repository guidance](AGENTS.md). The current migration is maintained on
`dev-reader-next`; the [implementation status](docs/development/sqlite-reader-next-status.md)
records completed checks and remaining release gates.

## License

Apache-2.0 OR MIT.
