# Renderer editor mutation admission checkpoint

Based on integration `811ceafebbd2446bb40c7c4b684c92b10d910a72`, this checkpoint
adds real renderer admission and pending-save ownership. No native preflight
command/capability or bridge mount is included. A held renderer gate or drained
promise queue is **not** proof of native all-writer preservation readiness.

The actual editor store routes seed, edit, mode, direct saved/conflict updates,
remove, retain and reset through one admission owner. Entry incarnations change
on reseed/removal; generations cover actual mutation and save lifetime. At most
64 saves may be issued, one per entry. The actual local/cloud AppShell save
handlers retain immutable buffer/anchor snapshots and release their leases in
`finally`, after the real promise settles. Pending saves remain owned even if a
hold is released. Under a hold, only the captured entry generation may adopt a
save/conflict completion. A replaced entry or duplicate/finished receipt cannot
alter the new buffer. Without a hold, ordinary typing during a save still adopts
the saved anchor while keeping newer text dirty. Errors retain buffers.

The actual CodeMirror pane synchronously rejects document changes while held,
including paste/undo/IME transactions, and updates editable/read-only/ARIA state
without replacing its view. Its save/mode shortcuts are also denied. All local
and legacy cloud source panes share the same renderer admission owner.

## Verification, 2026-10-08

All inputs are synthetic; existing dependency links were reused. The granted
frontend slot was explicitly released after these terminal checks.

- Seven new cases passed: actual store lifecycle 4, actual CodeMirror 1, and
  actual AppShell pending local/cloud save paths 2.
- Removing the real store admission guard made the exact retention case fail:
  ordinary reset deleted the held original buffers. The exact source was
  restored before final verification.
- Affected group **64 passed** in a 5.80 s Vitest run (4.47 s execution): new
  admission 4, CodeMirror 1, existing editor store 15, and full composed editor
  integration 44 (42 existing plus 2 new). No assertions/tests were removed.
- TypeScript passed in 2.35 s; seven-file scoped ESLint passed in 0.69 s.

Commands from `ui/`:

```sh
node_modules/.bin/vitest run src/features/editor-preservation/admission.test.ts src/shell/SourcePane.admission.test.tsx src/stores/editor.test.ts src/shell/edit-integration.test.tsx
node_modules/.bin/tsc --noEmit
node_modules/.bin/eslint src/features/editor-preservation/admission.ts src/features/editor-preservation/admission.test.ts src/stores/editor.ts src/shell/AppShell.tsx src/shell/SourcePane.tsx src/shell/SourcePane.admission.test.tsx src/shell/edit-integration.test.tsx
```

No Rust, bindings generation, model, browser, packaging, installation or broad
suite ran. The native preflight [contract proposal](../development/unsaved-editor-preflight-contract.md)
separates bounded metadata from actual local/cloud saved evidence. Preparation
must not silently apply the index's 2 MiB bound to existing local editing. Native
registration before registry seal, explicit hold-bound Save permission, real
receipt association, final authority verification and bridge mounting remain
separate reviewed work. An uncertain renderer IPC reply also does not establish
that an underlying native effect has stopped; native participants must own it.
