import { useEffect, useMemo, useRef, useState } from 'react';
import { parseMarkdownAuto } from '@mdreader/markdown/worker';
import type { ParsedDoc } from '@mdreader/markdown';

/** Only this draft can publish a parse; saved content is never a preview fallback. */
export function useDraftPreview(identity: unknown, markdown: string | undefined, enabled: boolean) {
  const request = useMemo(() => ({}), [identity, markdown, enabled]);
  const current = useRef(request); current.current = request;
  const [result, setResult] = useState<{ request: object; doc: ParsedDoc | null; error: string | null } | null>(null);
  useEffect(() => {
    if (!enabled || markdown === undefined) return;
    let active = true;
    const timer = setTimeout(() => {
      void parseMarkdownAuto(markdown).then(doc => {
        if (active && current.current === request) setResult({ request, doc, error: null });
      }).catch(() => {
        if (active && current.current === request) setResult({ request, doc: null, error: 'This draft could not be previewed. Your text is kept.' });
      });
    }, 150);
    return () => { active = false; clearTimeout(timer); };
  }, [request]);
  return result?.request === request ? { doc: result.doc, error: result.error } : { doc: null, error: null };
}
