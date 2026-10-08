# Proposed real UnsavedEditor preservation owner

This is a source-grounded proposal, not an implemented capability. The renderer
bridge remains unmounted. A clean `dirtyTabIds()` result cannot manufacture a
native UnsavedEditor participant or prove that future edits are denied.

## Existing ownership and ordering

`src-tauri/src/profile_transition/startup.rs::compose` currently registers the
two real JSON owners before renderer invokes. `registry.rs::register` rejects
replacement/wrong-root registration, and `admission.rs::enter` seals registration
on the first effect. Therefore an editor owner must be composed at startup,
before that seal; it cannot be registered later after a Settings preflight.
`Registry::with_frozen` checks all eight participants, denies/drains global
admission, freezes/revalidates the actual owners, and notifies release only after
explicit native resume. Ordinary document saves must finish **before** this
phase: running them in `verifyEditorFrozen` would deadlock/refuse global admission.

The actual renderer mutations are in `ui/src/stores/editor.ts`: `seed`, `setMode`,
`edit`, `markSaved`, conflict transitions, `removeTab`, `retain` and `reset`.
`AppShell.tsx` owns `setEditorMode`, local `saveDocEntry`/`saveActiveDoc`, legacy
cloud `saveCloudEntry`, `saveTabAll`, `saveAllDirty`, and the Save/Discard/Cancel
unsaved guard. Its existing Save handler rechecks guard identity and dirty buffers
after awaits; typing during a save deliberately stays dirty. The independent
native workspace/cloud writer participants must still cover their real writes.

## Smallest coherent next implementation

1. Compose a native `EditorOwner`/`LiveParticipant` at startup, bound to the same
   retained profile root and renderer registration, initially **unprepared**.
   Its freeze rejects absent/expired preparation. Keep its operation/owner epoch,
   exact root identity, renderer epoch, edit generation and deadline in native
   custody; never accept a renderer-supplied ready boolean as the capability.
2. Extract an editor preflight controller beside the existing editor store. An
   explicit preservation action first holds editor mutation admission and joins
   the actual in-flight local/cloud saves. Gate every listed entry point, not
   only keystrokes or the visible tab. Completion of an already issued save may
   settle only its captured entry/generation while the gate is held. Navigation,
   external reload/conflict updates and destructive cleanup must not replace the
   captured buffers during preflight.
3. Reuse the real Save/Discard/Cancel UI against all dirty entries, including
   inactive tabs and navigated-away buffers. Save uses existing exact-hash guarded
   writers and their actual completion/conflict results. Recheck generation and
   guard identity after each await. Discard is explicit per captured operation;
   do not erase retained buffers merely to obtain a clean count. Cancel/failure
   keeps originals and releases only this preflight's admission.
4. The native preflight handshake must bind an exact bounded entry snapshot and
   generation to the authenticated window/renderer. Native custody verifies saved
   entries against real guarded-write receipts/current authority, or retains the
   explicit discard decision for those exact entries. Renderer-provided buffer
   digests alone are insufficient proof of saved content. Bounds and receipt
   association must be reviewed against actual editor document limits before
   adding this API; do not silently truncate large/unknown entries.
5. Only after this preparation may the native preservation operation begin. Its
   UnsavedEditor freeze verifies/holds the prepared owner capability; the renderer
   bridge verifies that held capability rather than saving/discarding there.
   A stale epoch/root/window, typing/save conflict, timeout, unmount or uncertain
   native result cannot be treated as successful preparation or auto-unfreeze.
   Matching native release unwinds this exact operation; duplicate/stale release
   cannot reopen a new editor generation.
6. Mount the bridge through `use-app-persistence.ts` after actual hydration and
   editor capability composition. Its `latest()` uses that hook's existing tracker
   `peek` and the **same** `sessionWriter.snapshot`; read-only history is omitted.
   Resume re-enqueues current accumulated groups before consuming deltas, so an
   older failed ordinary save cannot later overwrite a successful scoped flush.
   This remains one native JSON writer; do not open another plugin store.

Suggested isolated modules: native `profile_transition/editor/{mod,commands,
participant}.rs`, renderer `features/editor-preservation/{controller,admission,
preflight}.ts`, and narrow startup/registration, editor-store and persistence-hook
composition. Existing generic profile command/binding shapes need separate review
before any edit. Keep AppShell as composition, not another native owner.

## Bounded acceptance before any readiness claim

Use a synthetic dirty local buffer plus an inactive dirty buffer and a pending
save. Prove startup registration precedes first-effect seal; unprepared owner
rejects freeze; a real Save/Discard decision admits only the exact generation;
typing/conflict/cancel during await retains buffers and refuses preparation;
late/stale/duplicate release cannot reopen a replacement operation. Check real
local guarded-write receipts and the actual native participant, not just a mocked
clean-list callback. Then test the mounted same-writer snapshot with interleaved
failed legacy/canonical saves and a future read-only namespace. The remaining
participant capabilities and final installed-profile export/rollback acceptance
remain required independently.
