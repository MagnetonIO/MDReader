/**
 * The watcher-driven reindex decision, factored out of the AppShell so its
 * ordering — "run an incremental import, THEN refresh the listing; unless an
 * import is already in flight, in which case just refresh" — is unit-testable
 * without a DOM or a Tauri backend.
 *
 * The engine's incremental import is cheap, so a settled filesystem change
 * drives a re-import (which keeps the index — and thus wikilink/title
 * resolution — in step with disk) and only then re-lists. While an import is
 * already running the backend coalesces duplicates (see `start_import`'s
 * per-root+generation handle), so we skip re-triggering and refresh the listing
 * alone.
 */

export interface ReindexDeps {
  /** Whether an import is already in flight (the backend coalesces duplicates). */
  importing: boolean;
  /** Run an incremental import for the granted root; resolves when it settles. */
  startImport: () => Promise<void>;
  /** Re-list the workspace files into the store. */
  refreshFiles: () => Promise<void>;
}

/**
 * Route a settled filesystem change: reindex (import → refresh) when idle, or a
 * refresh alone when an import is already running. Resolves to which path ran.
 */
export async function reindexOnChange(deps: ReindexDeps): Promise<'reindexed' | 'refreshed'> {
  if (deps.importing) {
    await deps.refreshFiles();
    return 'refreshed';
  }
  await deps.startImport();
  await deps.refreshFiles();
  return 'reindexed';
}
