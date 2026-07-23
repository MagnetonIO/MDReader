/**
 * The tab-retirement decision for a workspace switch, factored out of the
 * AppShell so its gating is unit-testable. Retiring the previous workspace's
 * open tabs (into that workspace's Recent) is authorized ONLY when the open
 * actually committed as the current workspace — a stale `openRoot` success (its
 * epoch superseded during the async listing) must NOT close the newer
 * workspace's tabs.
 *
 * Selection and attribution are BY EACH TAB'S OWN `originRoot`, never a snapshot
 * of the transient current root: `retireForeign(targetRoot)` closes exactly the
 * tabs whose origin differs from the newly-committed root and hands each back
 * with its origin, so overlapping switches (B then C) attribute each tab to the
 * workspace it truly came from, and a same-root re-open retires nothing.
 */
import type { DocRef } from '../stores/types.js';

export interface RetireDeps {
  /** Whether the open committed as the CURRENT workspace (false when superseded). */
  committed: boolean;
  /** The newly-committed workspace root — the "keep" root for the retire scan. */
  targetRoot: string | null;
  /**
   * The workspace being switched away from, used ONLY to attribute a legacy tab
   * that carries no origin of its own (pre-`originRoot` sessions).
   */
  fallbackRoot: string | null;
  /** Close every tab foreign to `currentRoot`, returning each with its origin. */
  retireForeign: (currentRoot: string | null) => { docRef: DocRef; originRoot: string | null }[];
  /** Record a retired ref under its originating workspace. */
  addRecent: (ref: DocRef, root: string) => void;
}

/** Retire the foreign tabs iff the switch committed as current. Returns whether
 *  any tab was retired. */
export function retireTabsAfterSwitch(deps: RetireDeps): boolean {
  if (!deps.committed) return false;
  const retired = deps.retireForeign(deps.targetRoot);
  for (const { docRef, originRoot } of retired) {
    const root = originRoot ?? deps.fallbackRoot;
    if (root !== null) deps.addRecent(docRef, root);
  }
  return retired.length > 0;
}
