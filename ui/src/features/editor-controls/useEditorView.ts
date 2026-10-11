import { useEffect, useState } from 'react';
import type { EditorMode } from '../../stores/editor';

/** Narrow layout changes presentation only; the chosen desktop view is retained. */
export function useEditorView(mode: EditorMode) {
  const [narrow, setNarrow] = useState(() => typeof matchMedia === 'function' && matchMedia('(max-width: 700px)').matches);
  useEffect(() => {
    if (typeof matchMedia !== 'function') return;
    const media = matchMedia('(max-width: 700px)');
    const changed = () => setNarrow(media.matches);
    changed(); media.addEventListener('change', changed);
    return () => media.removeEventListener('change', changed);
  }, []);
  return { narrow, view: narrow && mode === 'split' ? 'source' as const : mode };
}
