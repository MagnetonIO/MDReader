# Bounded native editor preflight contract proposal

No commands in this proposal are implemented. The renderer admission checkpoint
is separate; it does not mount the preservation bridge or establish native
editor custody. Native startup/registry changes require composition review.

## Actual saved evidence

Local `src-tauri/src/commands.rs::save_doc_guarded_with_hooks` owns the retained
workspace epoch/root, relative Markdown membership, advisory target lock, two
hash checks and temp/rename publication. Its public result is only
`saved { new_hash, mtime }` or `conflict { current_hash }`; there is currently no
native operation/entry receipt ledger. A lost reply may follow a real write.
`read_doc_guarded_with_hooks` uses `SourceRoot::read_relative`/`read_to_end`, and
local save receives an unbounded content string. The core index's 2 MiB limit
**does not** constrain this editing path. Preserve existing local behavior.

Legacy cloud `cloud_documents/service.rs::save_cloud_markdown` actually enforces
2 MiB, authenticates the edit token against library/source/pins/record/revision,
and persists repair/publication evidence under
`blake3("mdreader-cloud-write-v1\\0" || edit_token || actual_body_hash)`.
A provider-accepted repair may still require local publication. The public saved
result carries a new authenticated edit token and modification time, not the
repair key. `CloudAccountProfile::save_cloud_markdown` selects the real current
source owner; an error/unknown reply is not a saved receipt. Preflight must inspect
the actual local publication/repair state without initiating provider I/O.

## Proposed public metadata shapes

All requests use closed, internally tagged **struct** variants and authenticated
Tauri window + existing `RendererIdentity`. No roots, tokens, arbitrary bodies or
native custody objects are returned. Native-only registration may consume the
existing local guarded-read identity or authenticated cloud edit authority at
its existing command boundary; it must not treat a displayed path/title as a
grant. Handles are profile/window/renderer/incarnation-bound UUIDs, never inferred.

```ts
// Native-issued on actual editor seed/read; lifetime ends on replacement/reload.
type EditorEntryHandle = { entryId: string; entryEpoch: string };
type EditorPreparation = {
  preparationId: string; identity: RendererIdentity;
  editorGeneration: string; // decimal exact safe integer, no unchecked Specta u64
  entries: Array<{
    handle: EditorEntryHandle; generation: string;
    decision: { kind: 'unchanged' }
      | { kind: 'saved'; receiptId: string }
      | { kind: 'discard' };
  }>;
};
type EditorPreparationReceipt = {
  preparationId: string; identity: RendererIdentity;
  editorGeneration: string; entryCount: number;
};
```

A native-issued receipt ID must refer to actual guarded-write evidence retained
by the local/cloud owner for that handle/epoch. It is **not** a renderer-computed
buffer hash. The local write observer must record actual target, before/after
hash, workspace generation/root identity and known publication outcome inside
its existing owned effect; unknown publication cannot be silently called saved.
Cloud observation must associate the actual accepted-and-locally-published repair
and exact pins/version with the native entry. Expose only the opaque receipt ID
through a narrow result projection or an explicit receipt-reconciliation lookup;
review that integration before changing existing save DTOs. Do not add a second
durable body writer. `unchanged` requires the native-registered original anchor
and closed renderer generation; `discard` requires an explicit decision for that
exact captured entry, never a missing row or a clean-list inference.

Metadata packet limit: 64 KiB and 128 entries; UUIDs and generations strictly
validated. All registered live entries must be accounted for, including dirty
inactive/navigated-away entries; omission/duplicates reject. An existing larger
editor state remains usable but preparation reports unsupported/bounds and
retains every buffer. No implicit pruning, truncate or normalization.

## Ordering and retained ownership

1. Compose an initially unprepared UnsavedEditor owner before the registry's
   first-effect registration seal. Register actual renderer entry handles on
   seed/reseed and invalidate old incarnations, not merely on tab focus.
2. Explicit preparation holds renderer mutation admission, snapshots all entries
   and joins its issued save leases. Every store mutation and actual CodeMirror
   document change is denied. A captured completion may settle only the exact
   held entry generation; failed/unknown outcomes retain buffers. A lease survives
   cancellation until the real promise settles, and native write participants
   must independently retain any underlying effect after an uncertain IPC reply.
3. Save/Discard/Cancel happens before global profile denial. Explicit Save needs
   a hold-bound, exact-entry save permission, **not** reopening general editor
   admission. The current renderer checkpoint only drains already-issued saves;
   that scoped preflight-save permission and controller must be reviewed next.
   Cancel/failure releases only this preparation; it never erases pending work.
4. Native preparation verifies saved receipts plus actual retained membership and
   content outside owner locks. Proposed local verification budget: 32 MiB per
   file, 64 MiB total, 5 seconds for one bounded job, streaming fixed-size buffers.
   This budget constrains preparation only. Over-budget/unknown/changed content
   produces a fixed private-safe unsupported/stale diagnostic, retaining buffers.
   Avoid detached timeout threads: a timed-out worker remains owned until it
   settles. No filesystem/network/hash work under SQLite/native owner mutexes.
   Cloud verification uses exact cached publication/repair evidence, never fetch.
5. Reacquire short owner sections and recheck FIRST profile/root/window/renderer,
   preparation generation, entry incarnations and actual source/pins authority.
   Retain verification evidence for final freeze revalidation; an earlier hash
   alone is insufficient after an edit/revoke. Existing noncooperating external
   writers cannot be claimed frozen by the renderer gate/advisory lock.
6. Only the actual native owner may issue the preparation receipt. Freeze checks
   this exact held operation/epoch/root/deadline and revalidates its evidence;
   renderer `verifyEditorFrozen` verifies that held capability without ordinary
   saves. Matching release retires it; stale/duplicate release, unmount or unknown
   outcome cannot reopen another preparation or manufacture success.
7. Finally mount through the existing accumulated session writer as documented
   in the lifecycle receipt. All other profile participants remain required.

The proposed limits, local receipt observer and hold-bound Save permission need
review before native commands/startup/bindings are edited. No backup-ready or
installed-library preservation acceptance is claimed by this design.
