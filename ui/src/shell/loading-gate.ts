/**
 * Generation-aware guard for the sidebar `loading` flag, factored out of the
 * AppShell so its clearing rules are unit-testable. Two hazards it closes:
 *   - a STALE (older) listing completion must not clear a NEWER open's loading;
 *   - the NEWEST open failing before it ever lists must still clear loading
 *     (nothing else will), so the sidebar never gets stuck spinning.
 *
 * It tracks the generation that SET loading. A completion clears iff its
 * generation is that setter's or newer (`>=`); a failure force-clears.
 */
export interface LoadingSink {
  setLoading(loading: boolean): void;
}

export class LoadingGate {
  private setterGen = Number.NEGATIVE_INFINITY;

  /** A continuation under `generation` begins loading (records the setter). */
  begin(generation: number, sink: LoadingSink): void {
    this.setterGen = generation;
    sink.setLoading(true);
  }

  /** A continuation under `generation` completes; clears loading only when it is
   *  the setter's generation or newer — a stale completion cannot clear a newer
   *  open's loading. */
  complete(generation: number, sink: LoadingSink): void {
    if (generation >= this.setterGen) sink.setLoading(false);
  }

  /** The newest open failed before it could list — force-clear (no continuation
   *  will otherwise). */
  fail(sink: LoadingSink): void {
    sink.setLoading(false);
  }
}
