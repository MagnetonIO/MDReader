/**
 * The watcher/save-driven reindex coordinator, factored out of the AppShell so
 * its ordering AND its coalescing are unit-testable without a DOM or a Tauri
 * backend.
 *
 * Ordering: a settled change runs an incremental import THEN re-lists, so the
 * refreshed tree (and wikilink/title resolution) reflects the just-imported
 * change.
 *
 * Coalescing (the fix): the engine's import is single-writer, so a change that
 * lands WHILE an import is already in flight cannot be folded into that pass —
 * its content would be left out of the index until some unrelated later change.
 * The coordinator instead queues exactly one follow-up reindex: it refreshes the
 * listing immediately (responsive UI) and, once the active import settles, runs
 * one more import→refresh so the queued change is indexed. Further changes during
 * that follow-up coalesce into it (the loop drains `pending`), so a save is never
 * silently dropped from the index.
 */

export interface ReindexDeps {
  /** Run an incremental import for the current workspace; resolves when it (and
   *  any one-time model load) settles. */
  startImport: () => Promise<void>;
  /** Re-list the workspace files into the store. */
  refreshFiles: () => Promise<void>;
}

/** What a single `onChange` did. */
export type ReindexOutcome = 'reindexed' | 'queued';

export interface Reindexer {
  /**
   * A settled change (fs event or a save) landed. When idle: import → refresh.
   * When an import this coordinator owns is already running: refresh now and
   * queue exactly one follow-up reindex for when it settles. Resolves to which
   * path ran.
   */
  onChange: () => Promise<ReindexOutcome>;
}

export function createReindexer(deps: ReindexDeps): Reindexer {
  // Whether THIS coordinator's import pump is currently running. Because every
  // import is triggered through here, this alone tracks "an import is in flight".
  let running = false;
  // A change queued while the pump was running — drives exactly one rerun.
  let pending = false;

  async function pump(): Promise<void> {
    running = true;
    try {
      // Import → refresh; loop once more for every change that queued during a
      // pass (coalesced), so nothing lands after the single-writer import missed it.
      do {
        pending = false;
        await deps.startImport();
        await deps.refreshFiles();
      } while (pending);
    } finally {
      running = false;
    }
  }

  return {
    async onChange() {
      if (running) {
        // An import is already in flight — it cannot absorb this change. Refresh
        // for immediate feedback and queue exactly one follow-up reindex.
        pending = true;
        await deps.refreshFiles();
        return 'queued';
      }
      await pump();
      return 'reindexed';
    },
  };
}
