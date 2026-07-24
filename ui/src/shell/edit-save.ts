/**
 * The save/conflict decision logic, factored out of the AppShell so it is pure
 * and unit-testable without a DOM, a Tauri backend, or CodeMirror. Two concerns:
 *
 *   - `classifyExternalChange` — when the watcher reports our currently-edited
 *     document changed on disk, decide whether it is our OWN write (suppress the
 *     "Updated" toast), a benign reload (adopt disk), or a genuine conflict (the
 *     buffer is dirty, so raise the banner rather than clobber the edit). The
 *     `self` case is the self-write suppression the plan calls for: after a save
 *     the buffer's `baseHash` equals the freshly-written hash, so the watcher's
 *     echo of that very write hashes back to the same value.
 *
 *   - `runSave` — issue a hash-guarded (or forced) save and normalise its outcome
 *     into a small tagged result the shell maps onto store transitions. A backend
 *     conflict rides the `saved | conflict` union, never an exception; only hard
 *     failures land as `error`.
 */

/** The wire outcome of a `save_doc`, already unwrapped by the fs adapter. */
export type SaveOutcome =
  | { status: 'saved'; newHash: string; mtime: number }
  | { status: 'conflict'; currentHash: string };

/** What the watcher's report about our edited document means. */
export type ChangeClass = 'self' | 'reload' | 'conflict';

/**
 * Classify a disk change to the document currently open in the editor.
 *   - `self`     — disk matches our anchor: our own just-saved write (or a no-op
 *                  touch). Suppress the toast; nothing to do.
 *   - `conflict` — disk diverged AND we have unsaved edits: never clobber, raise
 *                  the banner.
 *   - `reload`   — disk diverged but the buffer is clean: safe to adopt disk.
 */
export function classifyExternalChange(
  diskHash: string,
  baseHash: string,
  dirty: boolean,
): ChangeClass {
  if (diskHash === baseHash) return 'self';
  return dirty ? 'conflict' : 'reload';
}

/** The normalised result of a save attempt. */
export type SaveResult =
  | { kind: 'saved'; newHash: string; mtime: number }
  | { kind: 'conflict'; currentHash: string }
  | { kind: 'error'; message: string };

export interface RunSaveDeps {
  path: string;
  /** The bytes to write (the live editor buffer). */
  buffer: string;
  /** The hash the buffer is anchored to — the guard the backend enforces. */
  baseHash: string;
  /** When true, bypass the guard (the user's explicit "overwrite on-disk"). */
  force: boolean;
  /** The fs adapter's `saveDoc`. */
  save: (
    path: string,
    content: string,
    expectedHash: string,
    opts: { force: boolean },
  ) => Promise<SaveOutcome>;
}

/**
 * Run a save and normalise its outcome. A `conflict` outcome is an expected
 * result (the file changed on disk), not an error; only a thrown backend/IPC
 * failure becomes `error`.
 */
export async function runSave(deps: RunSaveDeps): Promise<SaveResult> {
  try {
    const outcome = await deps.save(deps.path, deps.buffer, deps.baseHash, {
      force: deps.force,
    });
    if (outcome.status === 'saved') {
      return { kind: 'saved', newHash: outcome.newHash, mtime: outcome.mtime };
    }
    return { kind: 'conflict', currentHash: outcome.currentHash };
  } catch (cause) {
    return { kind: 'error', message: cause instanceof Error ? cause.message : String(cause) };
  }
}
