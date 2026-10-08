# MDReader for local VS Code

Read, preview, search, follow document references, inspect related links, and
check index status through the existing MDReader desktop service. The extension
opens revision-bound **read-only virtual documents**, so source jumps preserve
source ID, source-relative path, revision, and line evidence.

## Configure explicit access

Start the native MDReader service separately. In **User Settings**, configure:

```json
{
  "mdreader.dataDirectory": "/absolute/path/to/reader-data",
  "mdreader.sourceIds": ["ALREADY_REGISTERED_SOURCE_ID"]
}
```

The directory must be absolute; select 1–32 already registered source IDs.
Workspace settings cannot choose the profile or grant source access. The
extension never grants a folder, opens a database, runs embeddings, writes a
document, or launches a service writer. It reads the service's private endpoint
and delegates bounded authenticated requests over loopback. Missing service,
revoked source, and stale revisions produce sanitized errors.

Use these Command Palette commands:

- **MDReader: Search Selected Sources** returns up to 20 revision-bound results;
  choosing one opens its source-relative document at the evidence line.
- **MDReader: Open Document from Source** selects an allowed source and accepts a
  relative document path.
- **MDReader: Preview Reader Document** previews the active `mdreader:` document.
- **MDReader: Show Related Links** finds up to 100 incoming and outgoing linked
  documents. Results are a combined related set; exact backlink-only navigation
  remains pending.
- **MDReader: Show Source and Index Status** reports scoped document/chunk/model
  counts; it is also available from the status bar.

Preview links resolve through the same source and revision guard. Raw HTML is
escaped, rendering is sanitized, scripts/forms are disabled, and CSP blocks
resource loading. Images/assets are currently labeled without being loaded.
Standard Markdown, code, tables, and ordinary relative links render; shared
reader math/diagram rendering and wiki-link-specific preview controls are not
included in this slice. Preview metadata and link arguments are treated as data.

Changing configured scope closes preview panels and suppresses stale operations.
VS Code may retain already loaded virtual text in editor memory or its history;
close existing reader documents when removing a grant. Every subsequent service
read rechecks the current allowlist and the document revision.

## Supported host and acceptance limits

V1 supports **local desktop VS Code** only. Remote SSH/WSL/container/Codespaces
windows and web extension hosts are explicitly unsupported. There is no browser
entry point, and commands reject remote/web environments before endpoint access.
Ordinary build/unit tests do not install the extension or change client profiles.
The separate explicit acceptance driver installs the VSIX only into a disposable
local profile; it never publishes the extension or changes global configuration.

Automated checks cover transport framing and limits, source/path/output guards,
revision failures, cancellation/timeouts, preview injection, host gating,
user-only configuration, and stale async result suppression. A separate integration
test launches the real service and disposable Unix sources with the reader
window closed. A real macOS VS Code 1.141.0 Extension Host run also activated an
installed VSIX, read exact service content, created a preview, followed a reference,
and rejected stale revisions/unselected sources. See the
[bounded client receipt](../../crates/mdreader-mcp/validation/client-hosts-2026-10-08.md).
Interactive search/quick-pick flows and Windows/Linux host acceptance remain
separate release gates. Cancellation closes the client socket; current
service work does not have an in-flight cancellation API.

## Build and package

Use Node 24 and npm. Dependencies and package lock are local to this extension.

```sh
npm ci --ignore-scripts
npm test
npm run check
npm run package
```

`artifacts/mdreader-1.0.0.vsix` contains a bundled Node extension and third-party
license notices. Build output and VSIX artifacts are ignored by git. To run the
real-service test after building `mdreader-service` from the repository root:

```sh
MDREADER_TEST_SERVICE_BIN=/absolute/path/to/mdreader-service npm run test:service
```

Implementation follows the official VS Code documentation for
[extension hosts and remote environments](https://code.visualstudio.com/api/advanced-topics/remote-extensions),
[read-only virtual documents](https://code.visualstudio.com/api/extension-guides/virtual-documents),
and [webview security](https://code.visualstudio.com/api/extension-guides/webview).

## Explicit installed-client acceptance

The reusable macOS driver requires installed `claude`, `codex`, `code`, and
`/Applications/Visual Studio Code.app`, plus current standalone binaries and a
VSIX. It creates a small synthetic workspace, selected and denied sources, a
private no-model service, and isolated native client profiles. It installs and
uninstalls only its project MCP entries, verifies original bytes, and compares
the known global configuration files before/after without logging their contents.
It neither copies credentials nor sends a model turn.

From the repository root, build the narrow native binaries in an assigned build
slot (reuse the compatible target cache), then package from this directory:

```sh
CARGO_BUILD_JOBS=2 cargo build --locked --offline -p mdreader-service -p mdreader-mcp -p mdreader-capture
cd extensions/vscode
npm run package
node scripts/acceptance/run.js \
  --service /ABS/TARGET/debug/mdreader-service \
  --mcp /ABS/TARGET/debug/mdreader-mcp \
  --capture /ABS/TARGET/debug/mdreader-capture \
  --vsix /ABS/REPO/extensions/vscode/artifacts/mdreader-1.0.0.vsix \
  --output /ABS/PRIVATE/client-receipt.json
```

Use `--clients claude`, `--clients codex,mcp`, or `--clients vscode` for an
affected phase. Success prints only phase results and removes its generated
workspace/profile. Failure retains the exact synthetic diagnostic directory
printed on stderr; inspect it privately, stop any remaining fixture child, then
remove only that directory. Missing installed clients fail rather than download
anything. macOS needs a short temporary path for the VS Code socket; this driver
uses a private `/tmp/mdreader-clients-*` root.

Claude approval and Codex project trust are explicit settings in disposable
client profiles. VS Code workspace trust is disabled for this explicit synthetic
fixture; interactive trust dialogs are not accepted by this run. The installed
MDReader VSIX is distinct from the tiny development test-driver extension.
A synthetic launcher routes app-style flags to standalone binaries: a successful
run does **not** verify the packaged desktop entrypoint, global installation,
real user projects, authenticated model turns, or final-output hooks.
