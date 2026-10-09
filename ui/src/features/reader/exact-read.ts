const RETRY_DELAYS = [100, 200, 400, 800, 1200] as const;
function aborted(): DOMException { return new DOMException('The reading selection changed.', 'AbortError'); }
function wait(milliseconds: number, signal: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal.aborted) { reject(aborted()); return; }
    const timer = setTimeout(() => { signal.removeEventListener('abort', cancel); resolve(); }, milliseconds);
    function cancel() { clearTimeout(timer); signal.removeEventListener('abort', cancel); reject(aborted()); }
    signal.addEventListener('abort', cancel, { once: true });
  });
}

/** Bounded owner-contention retry only. The caller closes over one immutable
 * receipt and admits only its known native Busy error; no fallback revision,
 * provider request, credentials or authorization operation is created here. */
export async function readExact<T>(read: () => Promise<T>, signal: AbortSignal,
  retryable?: (cause: unknown) => boolean, waiting?: () => void): Promise<T> {
  for (let attempt = 0; ; attempt++) {
    if (signal.aborted) throw aborted();
    try {
      const value = await read();
      if (signal.aborted) throw aborted();
      return value;
    } catch (cause) {
      if (signal.aborted) throw aborted();
      const delay = RETRY_DELAYS[attempt];
      if (delay === undefined || !retryable?.(cause)) throw cause;
      waiting?.();
      await wait(delay, signal);
    }
  }
}
