import { useMemo, useRef } from 'react';
import type { RefObject } from 'react';
import type { MarkdownReaderHandle } from '@mdreader/ui';

/** One restoration attempt per exact parsed selection. Programmatic movement
 * cannot overwrite its saved position before live alignment; deliberate user
 * scrolling cancels that attempt and resumes ordinary position capture. */
export function useReaderRestoration(reader: RefObject<MarkdownReaderHandle | null>, failed: () => void) {
  const state = useRef({ generation: 0, phase: 'idle' as 'idle' | 'pending' | 'ready' | 'failed' });
  const notify = useRef(failed);
  notify.current = failed;
  return useMemo(() => ({
    reset() {
      state.current.generation++;
      state.current.phase = 'idle';
      reader.current?.cancelScrollNavigation();
    },
    attach(handle: MarkdownReaderHandle | null, line: number, heading: string | null) {
      reader.current = handle;
      if (!handle || state.current.phase !== 'idle') return;
      const generation = state.current.generation;
      state.current.phase = 'pending';
      const settled = (aligned: boolean) => {
        if (state.current.generation !== generation || reader.current === null || state.current.phase !== 'pending') return;
        state.current.phase = aligned ? 'ready' : 'failed';
        if (!aligned) notify.current();
      };
      // An absent fragment falls back to the explicit source line. Its missing
      // callback must not finalize this restoration before that fallback runs.
      let requested = false;
      if (heading) requested = handle.scrollToFragment(heading, aligned => { if (requested || aligned) settled(aligned); });
      if (!requested) handle.scrollToLine(line, settled);
      return heading ? requested : undefined;
    },
    userIntent() {
      if (state.current.phase === 'pending' || state.current.phase === 'failed') {
        state.current.phase = 'ready';
        reader.current?.cancelScrollNavigation();
      }
    },
    canSave() { return state.current.phase === 'ready'; },
  }), [reader]);
}
