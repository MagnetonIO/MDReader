/**
 * The tab-retirement decision for a workspace switch, factored out of the
 * AppShell so its gating is unit-testable. Retiring the previous workspace's
 * open tabs (into that workspace's Recent) is authorized ONLY when the open
 * actually committed as the current workspace — a stale `openRoot` success (its
 * epoch superseded during the async listing) must NOT close the newer
 * workspace's tabs.
 */
import type { DocRef } from '../stores/types.js';

export interface RetireDeps {
  /** Whether the open committed as the CURRENT workspace (false when superseded). */
  committed: boolean;
  /** The workspace being switched away from, or `null` on a first open. */
  previousRoot: string | null;
  /** Close all open tabs, returning the retired refs. */
  closeAll: () => DocRef[];
  /** Record a retired ref under its originating workspace. */
  addRecent: (ref: DocRef, root: string) => void;
}

/** Retire the previous workspace's tabs iff the switch committed as current.
 *  Returns whether tabs were retired. */
export function retireTabsAfterSwitch(deps: RetireDeps): boolean {
  if (!deps.committed || deps.previousRoot === null) return false;
  const retired = deps.closeAll();
  for (const ref of retired) deps.addRecent(ref, deps.previousRoot);
  return true;
}
