# Native incoming preparation

`incoming::prepare` is a native, synchronous, memory-only boundary between
encrypted staging and future owner projection. It opens no database, reads no
files, performs no network requests, exports no keys and cannot advance received,
applied or server acknowledgment. Default-off product sync remains unchanged.
Success is not an issuer/device grant, selected-library grant, consent, conflict
resolution or an applied receipt.

The native caller supplies the exact scope, staged root, up to 16 custody-held
key references and downloaded ciphertext dependencies. Current verified transport
authority and owner selection must be revalidated after network/OS waits before
publication. Account identity is not inside AAD v1: verified account scope and
client-only key custody remain separate necessary trust boundaries.

## Admission

- Existing AES256GCM envelope/AAD v1 is unchanged. Root and each chunk verify exact
  envelope SHA256; each envelope independently selects a retained library key.
- Root document/revision must match encrypted content. Chunk AAD binds descriptor
  item/revision, and chunk payload document/revision/role/path/index/count must
  match. Raw chunk length/hash and reassembled length/hash must match descriptors.
- Missing keys/bodies return `NotReady`; extra/duplicate bodies, malformed inputs,
  mismatches and bounds fail without a partial result. Download order has no authority.
- Existing codec limits remain 1 MiB Markdown, eight PNG/JPEG assets, 4 MiB per
  asset and 16 MiB total assets. Additional ciphertext limits are 72 dependencies,
  512 KiB each and 32 MiB total before original-byte allocation.
- UTF-8 is checked after joining chunks, preserving split codepoints, BOM, CRLF
  and Unicode. Titles/paths/content are not normalized. Paths retain strict relative
  codec authority; no arbitrary filesystem access is accepted.
- PNG/JPEG signatures and structure are checked, with dimensions at most 4096
  and 4,194,304 pixels. PNG animation/compressed metadata and unsupported JPEG
  frame types are rejected. This is structural MIME admission, **not pixel decoding**
  or renderability proof; the existing bounded native decoder is still required.
- User-state line and block/pixel positions stay distinct. Target existence,
  dirty state, stale-position clearing and pin preservation belong to projection.
  No approximate fraction/line/block conversion is made.
- `OpaqueTombstone` remains untyped: wire deletion has no payload kind. Only the
  durable item association may distinguish document from user-state deletion.
  Unknown deletion must remain unresolved.

Prepared values have private constructors and no Debug/Clone/Serialize. Original
bytes and selected title/path buffers use Zeroizing. Temporary serde allocations,
caller copies, allocator remnants and OS memory are not guaranteed erased. This
is best-effort memory hygiene. Plaintext must never enter renderer RPC, telemetry,
URLs or transport logs.

## Coordinated owner gate

This slice contains no mutation callback or `mark_applied` call. Selected imports
and assets currently use app-owned durable files/metadata. Extract that ownership
into core with versioned migrations preserving grants/revocation, pending intents,
pins and positions; do not add parallel permanent sync documents or dual writes.
Remote UUID revisions and native content-hash revisions need explicit mapping.

Owner-reserved, fsynced exact files precede atomic publication. One owner SQLite
transaction must commit originals/assets metadata, local edit/state CAS, verified
remote parent associations, projection receipts and the contiguous applied cursor.
Failure rolls back those writes; reopen reconciles persisted intents only.
Rebuildable indexing follows commit. Conflicts preserve local originals/pins and
hold applied; document tombstone authority must survive conflicting deletion to
block old user-state resurrection. Revocation/scope changes invalidate publication.
Only actual durable applied receipts permit acknowledgment.

Dependency network fetching, portable owner projection, conflict UI, complete
native custody/pairing/recovery, real issuer and mobile acceptance remain gates.

## Evidence

Seven focused incoming tests use unchanged public browser fixtures, generated
synthetic ciphertext and temporary SQLite. They cover exact originals/positions,
wrong key/tamper/context, chunk metadata/raw/full hashes, UTF-8/raster rejection,
opaque tombstones and unchanged cursors across reopen, rotated keys, dependency
bounds and split UTF-8 across two chunks with reversed download order. One raster
unit test covers truncation, dimensions, animation and unsupported JPEG frames;
its JPEG skeleton proves structural admission only, not image decoding.

Tests were authored before production code, but code-only sequencing meant no
preimplementation behavioral RED was observed. Initial compilation found a Rust
slice type mismatch, corrected before the focused suite passed. The affected crate
suite passed 39 entries with five explicit local Worker/issuer cases ignored.
Strict Clippy for all affected targets/features and Rust formatting passed.

Run only with the coordinator's build slot and compatible cache, jobs 2:

```sh
cargo test -p mdreader-sync --test incoming --locked --offline
cargo test -p mdreader-sync --features native-authority --locked --offline
cargo clippy -p mdreader-sync --features native-authority --all-targets --locked --offline -- -D warnings
```

No dependencies, protocol changes, deployment, real profiles or live accounts were
involved. Existing Worker tests are not new projection acceptance.
