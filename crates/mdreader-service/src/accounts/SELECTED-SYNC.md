# Explicit selected-document sync

Settings supplies a closed receipt for the currently opened **On this device**
document: document UUID, retained source SHA256 identity and exact raw Markdown
BLAKE3 revision. It supplies a stable operation UUID and the current verified
account digest / library UUID / signing-key RFC7638 device thumbprint. These are
opaque identifiers, not credentials or a source grant. Device thumbprints are
canonical 43-character base64url SHA256 values, not UUIDs.

`AccountRequest::SyncSelected` serializes through the actual account actor. It
uses the existing native authority, retained blocking projection lease,
`IncomingCoordinator`, configured document/assets roots and foreground pump.
The receipt is checked before expensive file work and again against the retained
archive/current snapshot during atomic graph admission. An unseen newer revision
cannot be substituted for the selected one. The existing final source, archive,
asset, generation and session fences remain in force. Queued-only `SyncNow`
remains separate. There is no automatic selection, upload or retry.

A selected job retains its exact identity independently of subsequent navigation.
`InspectSelected` is a readonly action behind earlier actor work. It can report
absent, queued, applied with exact sequence, or reauthorization required. Applied
requires matching durable association, typed incoming receipt, staged metadata,
applied cursor and exact immutable successful commit receipt; a completion label
or counter cannot settle a dispatched request. Inspection does not decrypt
bodies, initialize namespaces, repair schemas, access the vault or authorize a
new upload. Missing, malformed and future schemas fail closed.

## UI retry identity

Before dispatch, the renderer retains only a strict versioned bounded opaque
identity in localStorage (eight account/library/device scopes, 16 KiB total).
All cooperating storage mutations use one origin-scoped exclusive Web Lock;
only synchronous storage work runs under it. Unchanged values do not rewrite
storage. A missing lock API or a busy lock disables selected upload honestly;
local reading remains available. Web Locks coordinate cooperating windows; they
do not make renderer storage trustworthy or replace native authority checks.
See the [Web Locks specification](https://www.w3.org/TR/web-locks/).

Current verified scope is rechecked after asynchronous lock admission before
IPC dispatch. Navigation, component unmount, lost replies, cancellation and
account changes retain uncertain identities. Unknown or malformed stored bytes
are preserved. Another account's identity is never submitted in the new scope.
Only the exact same-actor absent/applied inspection settles it; generic job
completion never does. Explicit retry reuses the same operation UUID. Native
custody and source authority remain authoritative even if renderer storage is
modified or lost.

## Evidence and remaining gates

See [the focused acceptance receipt](acceptance/README.md). The local HTTPS case
uses the production RPC/actor/OIDC/PKCE/Worker/SQLite/R2 flow with synthetic public
identity and the production vault encoding through a process-memory test keyring.
It does not exercise a live provider, host OS keychain, installed UI or mobile.
UI behavioral tests mock only the IPC/lock boundary and do not prove cross-host
WebView lock support.

Only explicitly selected Markdown and tracked PNG/JPEG content are uploaded.
Existing same-document echo preserves pins and exact line positions; subsequent
pin/position changes are not uploaded yet. Outgoing user-state and tombstones,
restart/new-login reauthorization of an old admitted graph, multiple-key policy,
cleanup/history/account export and real installed/live-account acceptance remain
separate gates. An old admitted graph is retained, never silently resealed or
reauthorized.
