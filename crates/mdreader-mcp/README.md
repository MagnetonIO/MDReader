# Read-only MDReader MCP

This stdio adapter delegates every library operation to the running
`mdreader-service`; it never opens SQLite, grants a filesystem path, indexes a
source, or starts a writer. Start the headless service separately. Supply an
absolute service data directory and repeat `--source` for each explicitly
selected, already registered source ID:

```sh
mdreader-mcp --data-dir /absolute/reader-data --source REGISTERED_SOURCE_ID
```

The adapter exposes exactly `list_sources`, `index_status`, `search`,
`read_document`, `related`, and `resolve_reference`. Omitted search/status source
filters use only the configured allowlist; explicit filters must be nonempty
subsets. Source IDs in results are checked again, and source registration paths
are never returned. Search evidence carries source/path/revision and line bounds;
reads add whole-document line bounds; references carry revision and a one-based
line. Passing `revision` to `read_document` makes stale reads fail.

Successful display text is prefixed through `mdreader_capture::mark_retrieval`.
Result metadata and structured-content metadata carry the same
`mdreader/retrieval` marker. Verbatim display or serialized structured output
then remains identifiable by the capture adapters as retrieval echo, preventing
an automatic recapture loop. Authoritative `data.markdown`, search snippets,
source revisions and line evidence remain unchanged. Removing the marker or
paraphrasing a retrieved body removes that explicit provenance; such output
cannot be guaranteed excluded. Newly authored visible plans remain capturable;
this frontend does not inspect hidden reasoning or infer authorship from text
similarity.

Protocol is pinned to **2025-11-25**. An initialize request for another version
receives 2025-11-25 so the client can accept or disconnect. Tools become available
after `notifications/initialized`. Messages use newline-delimited UTF-8 JSON,
with a 64 KiB input frame budget and a 16 MiB output frame budget. Malformed JSON
returns a parse error and leaves the connection usable. Oversized or incomplete
frames return a parse error and close the connection. Stdout contains only
JSON-RPC; CLI diagnostics use stderr and do not echo supplied paths.

Supported protocol surface is deliberately tools-only: no resources, prompts,
sampling, client filesystem roots, background tasks, or server notifications.
Calls are sequential and use the service's bounded transport timeouts. In-flight
service calls do not currently implement cancellation; cancellation notifications
are accepted without a reply. Core lexical search and linking work with model
unavailable; this adapter does not initialize/download an embedding model.

Synthetic integration tests launch real stdio and service OS processes against
temporary Markdown roots; no private session is read. A separate explicit
[installed-client acceptance driver](../../extensions/vscode/README.md#explicit-installed-client-acceptance)
installs synthetic project configuration in disposable client profiles. The
[2026-10-08 receipt](validation/client-hosts-2026-10-08.md) records real Claude
connection/approval, Codex tool discovery and an exact MCP read, and installed
VSIX activation. It does not claim model-turn, hook or packaged-app acceptance.

```sh
cargo test -p mdreader-mcp
```

Protocol sources verified on 2026-10-07:

- [stdio transport](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports)
- [lifecycle and negotiation](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle)
- [tools and errors](https://modelcontextprotocol.io/specification/2025-11-25/server/tools)
