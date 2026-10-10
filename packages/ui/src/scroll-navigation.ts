/** Align against live DOM coordinates, not virtual spacer estimates. The
 * column's padding/frontmatter and measurement changes affect every jump. */
export type ScrollSettlement = (aligned: boolean) => void;

interface ScrollTarget {
  virtualized: boolean;
  mount(): void;
  element(): HTMLElement | null;
  offset?: number;
  place?(offset: number): void;
  stop?(): void;
  settled?: ScrollSettlement;
}

export function createScrollNavigation(container: () => HTMLElement | null) {
  let active: { frame: number | null; timer: ReturnType<typeof setTimeout> | null; settled?: ScrollSettlement; stop?: () => void } | null = null;
  const finish = (job: NonNullable<typeof active>, aligned: boolean) => {
    if (active !== job) return;
    active = null;
    if (job.frame !== null) cancelAnimationFrame(job.frame);
    if (job.timer !== null) clearTimeout(job.timer);
    if (!aligned) job.stop?.();
    job.settled?.(aligned);
  };
  const cancel = () => { if (active) finish(active, false); };
  return {
    cancel,
    get active() { return active !== null; },
    start(target: ScrollTarget) {
      cancel();
      const job = { frame: null as number | null, timer: null as ReturnType<typeof setTimeout> | null, settled: target.settled, stop: target.stop };
      active = job;
      let frames = 0, stable = 0, previous: number | null = null;
      const align = () => {
        if (active !== job) return;
        job.frame = null;
        const scroll = container(), element = target.element();
        if (!scroll) { finish(job, false); return; }
        if (element) {
          const desired = scroll.scrollTop + element.getBoundingClientRect().top - scroll.getBoundingClientRect().top + (target.offset ?? 0);
          // At the end of a document, browser clamping is a successful nearest
          // reachable location. Zero jsdom dimensions are not real bounds.
          const max = scroll.scrollHeight > 0 && scroll.clientHeight > 0
            ? Math.max(0, scroll.scrollHeight - scroll.clientHeight) : Infinity;
          const expected = Math.min(max, Math.max(0, desired));
          if (target.place) target.place(expected);
          else scroll.scrollTop = expected;
          const placed = Math.abs(scroll.scrollTop - expected) <= 1;
          stable = placed && previous !== null && Math.abs(previous - expected) <= 1 ? stable + 1 : 0;
          previous = expected;
          if (!target.virtualized || stable >= 2) { finish(job, placed); return; }
        } else if (!target.virtualized) { finish(job, false); return; }
        if (++frames >= 24) { finish(job, false); return; }
        job.frame = requestAnimationFrame(align);
      };
      // A hidden/suspended WebView must not leave an autosave fence forever.
      job.timer = setTimeout(() => finish(job, false), 750);
      target.mount();
      if (target.virtualized) job.frame = requestAnimationFrame(align);
      else align();
    },
  };
}
