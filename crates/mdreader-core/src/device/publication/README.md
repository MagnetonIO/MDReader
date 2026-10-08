# Incoming new-document publication

This core slice prepares and publishes **new** documents on the existing device
owner. It does not apply replacements, user state or tombstones, authenticate sync
accounts, discover profiles, advance cursors, acknowledge receipts or activate sync.
Native/service integration and mobile/Windows acceptance remain separate gates.

## Owner contract

The native owner supplies a verified scope digest, remote item/revision UUIDs,
and fresh UUID activation/selection epochs in `IncomingOrigin`. Core compares
these exact fences; it does not authenticate caller claims. The application must
hold its selected-owner operation lease, revalidate current authority after waits,
and fence invalidation through the final transaction commit. Remote revision UUIDs
remain separate from the native raw-byte BLAKE3 revision and fresh local document UUID.

1. Build `IncomingDocument`/`IncomingAsset` outside the owner mutex. Exact UTF-8
   bytes, including BOM/CRLF/Unicode, are preserved. Names must be plain `.md`
   filenames; selected image targets must match Markdown AST image destinations.
2. `reserve_new` durably records the incoming origin and quota reservation before
   generated files. It reserves assets in the existing asset table, captures the
   generated document root and persists its identity. Exact retry preserves the
   local UUID; changed input, completed or dropped origins cannot regrant it.
3. Release owner access. `WritePlan::write` writes/fsyncs known generated names,
   accepts only an exact interrupted prefix, then reads/verifies retained files.
   `PreparedIncomingCopy::validate` fully decodes PNG/JPEG under one runtime
   validation permit outside the owner mutex. The permit survives until Ready drops.
4. Reacquire current selected-owner authority. `with_publication` verifies the
   live owner and origin, then supplies a scoped, non-escaping `PublicationGuard`
   and the exact borrowed owner connection. The application attaches its sync
   store to this connection; core opens no connection and owns no second writer.
5. Inside the caller's transaction, `publish_new` publishes the source grant,
   original document, validated assets, completed incoming intent and the existing
   published import-intent row. The latter preserves container-relocation recovery.
   Persist associations, receipt and cursor in the **same** transaction.
6. Run `revalidate_publication` after all callback/receipt/cursor writes,
   immediately before commit. Any error requires rollback of the entire transaction.
   Indexing follows commit; returned documents remain `indexed=false` until then.

A direct owner transaction has this shape (the service's real sync callback must
also perform its own authority and contiguous-cursor checks):

```rust,ignore
let receipt = with_publication(&mut owner, &ready, &current_origin, |connection, guard| {
    let transaction = connection.transaction()?;
    let receipt = publish_new(&transaction, &guard, &ready)?;
    // Persist exact association, receipt and cursor here using transaction.
    revalidate_publication(&transaction, &guard, &ready)?;
    transaction.commit()?;
    Ok::<_, PublicationError>(receipt)
})??;
// Schedule rebuilding/indexing after durable commit.
```

Final checks cover live-owner identity, exact borrowed connection, unchanged
origin/intent/document/import/source fields, enabled source, private-cache
separation, asset rows/initial manifest generation and retained private roots.
Files are reopened by their exact relative pathname and compared with retained
opened objects using device/inode/size/mtime/ctime evidence. Original bodies are
not rehashed or pixel-decoded while holding the owner transaction.

## Durability, bounds and recovery

`migrations/001-incoming-intents.sql` adds only the independently guarded
`device_incoming_schema_v1` and `device_incoming_intents_v1` staging namespace.
Existing document/import/asset migration history, library version 4 and index
version 5 remain unchanged. Future or malformed incoming layouts fail closed.

The existing 1 MiB/document, 1,000-document/64 MiB library quota includes local
and incoming reservations, including dropping intents. Assets retain eight/
16 MiB per document, 4 MiB per image, 512 items/32 MiB library, decoder and runtime
bounds. A separate 4,096-intent history cap preserves completed/dropped origin
fences; `IntentLimit` requires deliberate future maintenance, not silent pruning.
The namespace/quota helper compiles without the optional image decoder.

Ordinary `recover_imports` never considers reserved incoming intents. Explicit
`resume` selects one persisted intent/origin and returns a `WritePlan`; `read`
performs bounded file reads outside the owner mutex. Files alone confer no grant.
Completed replay does not reactivate a source: reconcile the caller's committed
association/applied receipt, rather than invoking reservation/publication again.

Explicit discard is staged: `prepare_drop` durably fences publication and retires
reserved assets; `DropPlan::remove_files` removes/fsyncs only known generated
copies; `finish_drop` releases quota and retains a dropped-origin fence. The caller
must quiesce issued writers under its operation lease before cleanup. Unknown
files and empty ungranted document leaves are retained; there is no recursive
cleanup. Failed cleanup retains its reservation. No automatic cleanup is enabled.
Older clients cannot count this new namespace: resolve/export active intents
before downgrading an incoming profile.

Unix file evidence is implemented. Windows/non-Unix incoming publication refuses
reservation/resume/cleanup before file mutation with `UnsupportedPlatform` until equivalent retained-file identity/change evidence is implemented;
existing source-root Windows support alone does not establish this acceptance.

## Focused evidence

Five real temporary-SQLite tests passed: exact originals/assets plus outer rollback
and reopen; interrupted copies/local recovery isolation and explicit resume; epoch,
wrong-connection and local-ID collision fences; post-callback source revocation,
private-cache overlap and asset-generation changes; file/root changes and pixel
decode refusal; shared quota, staged discard and future namespace preservation.
Several assertions share cohesive lifecycle tests; this is five tests, not a
claim of separate release scenarios. Tests followed code-only sequencing; no
preimplementation behavioral RED was observed.

With the coordinator's sole build slot and `CARGO_BUILD_JOBS=2`:

```sh
cargo test --locked -p mdreader-core --features device-assets --lib device::publication::tests
cargo clippy --locked -p mdreader-core --all-targets --features device-assets -- -D warnings
```

Observed: five tests passed (0.18 s); strict Clippy passed (2.44 s). Default-core
integration, existing asset behavior tests after decoder factoring, native
projection/cursor composition, packaging and live release acceptance are not
claimed by this focused checkpoint. No dependencies, lock changes, private
profiles or live sync operations were involved.
