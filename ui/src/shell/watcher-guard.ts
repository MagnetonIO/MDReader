/**
 * A debounced watcher timer is stamped at event ARRIVAL with BOTH the open
 * generation and the watcher generation, and re-validates BOTH when it fires.
 * The arrival-only check is insufficient: a timer can fire under a newer epoch
 * and re-tag old work, so a switch between arrival and fire (either counter
 * advancing) must discard the timer's action. Factored out so the ordering is
 * unit-testable without the DOM.
 */
export interface WatcherStamp {
  /** `openEpochRef` value captured at event arrival. */
  openGen: number;
  /** The event's `generation` (the watcher era) captured at arrival. */
  watcherGen: number;
}

/** Whether a timer stamped at arrival is still current at fire — BOTH the open
 *  generation and the watcher generation must be unchanged. */
export function watcherStampCurrent(stamp: WatcherStamp, current: WatcherStamp): boolean {
  return stamp.openGen === current.openGen && stamp.watcherGen === current.watcherGen;
}
