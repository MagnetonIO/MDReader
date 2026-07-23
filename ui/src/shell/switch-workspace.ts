/**
 * The workspace-switch flow, extracted from the AppShell so its two subtle
 * guarantees are unit-testable against injected deps:
 *
 *  1. a same-root request STILL supersedes — `openRoot` is invoked
 *     unconditionally (no `previousRoot === folder` early return), so a user
 *     pick of the workspace a session-restore open is still settling on bumps
 *     the epoch and refreshes rather than being skipped;
 *  2. retirement is by each tab's OWN origin against the newly-committed root
 *     (read post-commit), never a snapshot of the transient current root, so
 *     overlapping switches attribute tabs to the workspace they came from.
 */
import { retireTabsAfterSwitch } from './switch-retire.js';
import type { DocRef } from '../stores/types.js';

export interface SwitchDeps {
  /** Open (or supersede) the workspace; resolves whether it committed as current. */
  openRoot: (folder: string) => Promise<boolean>;
  /** The root being switched away from (a fallback for legacy null-origin tabs). */
  previousRoot: string | null;
  /** The current workspace root, read AFTER the open commits (authoritative target). */
  currentRoot: () => string | null;
  /** Close every tab foreign to a root, returning each with its origin. */
  retireForeign: (currentRoot: string | null) => { docRef: DocRef; originRoot: string | null }[];
  /** Record a retired ref under a workspace. */
  addRecent: (ref: DocRef, root: string) => void;
}

/** Perform a workspace switch. See the module docs for the two guarantees. */
export async function performSwitch(folder: string, deps: SwitchDeps): Promise<boolean> {
  // ALWAYS open — no early return on a same-root request (guarantee 1).
  const committed = await deps.openRoot(folder);
  // Retire by origin against the newly-committed root (guarantee 2).
  retireTabsAfterSwitch({
    committed,
    targetRoot: committed ? deps.currentRoot() : null,
    fallbackRoot: deps.previousRoot,
    retireForeign: deps.retireForeign,
    addRecent: deps.addRecent,
  });
  return committed;
}
