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
No extension install, user configuration change, publishing, or live GUI
acceptance is performed by the build/tests.

Automated checks cover transport framing and limits, source/path/output guards,
revision failures, cancellation/timeouts, preview injection, host gating,
user-only configuration, and stale async result suppression. A separate integration
test launches the real service and disposable Unix sources with the reader
window closed. Windows admission and macOS/Windows/Linux VS Code GUI acceptance
are separate release gates. Cancellation closes the client socket; current
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
