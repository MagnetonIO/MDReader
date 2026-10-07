# Native reader owner

`mdreader-service [--no-model] DATA_DIRECTORY` runs one authenticated loopback
owner. The packaged desktop binary exposes the same service with
`MDReader --mdreader-service DATA_DIRECTORY`. An OS file lock excludes competing
writers. Closing the launching window does not stop the child service.

The endpoint file is a private local capability, not a public HTTP API. Clients
must use the matching protocol and capability. Connections, input/output size,
and handler concurrency are bounded. Local VS Code and MCP clients delegate to
this service; they neither open the databases nor acquire source grants.

The service owns `library.sqlite` and disposable `index.sqlite`. Background jobs
are queued durably before acknowledgment and recovered after restart. Lexical
indexing precedes optional BGE initialization. `--no-model` prevents initialization
and model downloads for offline-only use and deterministic subprocess tests.
Model-unavailable status leaves reading and lexical search usable. The model
adapter is the existing 384-dimensional BGE contract; no substitute model is
silently installed.

Capture requires explicit `ConfigureCapture` for each project/worktree policy.
The service persists filesystem-object identity snapshots. Hooks write bounded
private events to `DATA_DIRECTORY/captures`; the service revalidates current
grants, projects ordinary Markdown, atomically admits provenance and an index
job, then acknowledges the spool occurrence. It never acknowledges solely on
projection or waits for model success before durability. Disabled/replaced roots
and revoked capture sources retain their pending events. Capture is not approval.
`CaptureStatus` reads diagnostics without consuming events. No user agent
configuration or private history is automatically discovered or modified.

Remaining release gates include capture installation/settings UI, cancellable
in-flight transport, Windows spool ACL/reparse-safe implementation, model
initialization time budgets, verified mobile model lifecycle, cloud identity and
pairing, and live desktop/mobile/extension acceptance. The presence of this crate
is not release acceptance.
