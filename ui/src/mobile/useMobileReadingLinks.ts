import { useCallback, useEffect, useRef, type MutableRefObject } from 'react';
import type { DeviceDocument } from '../bindings';
import type { CanonicalReaderReference } from '../features/reader/contracts';
import { nativeProvidersBackend, type ProviderReaderReference } from '../platform/providers';
import { providerTarget } from '../shell/provider-reading';
import { observeReadingLinks } from './document-transfer';
import { useReaderBackend } from '../platform/backend';

interface Handlers {
  openDevice(document: DeviceDocument, line: number): Promise<void>;
  openProvider(reference: ProviderReaderReference): Promise<void>;
  failed(message: string): void;
}
/** Paste and OS delivery share exact native read validation. A locator never
 * fetches an uncached provider body, creates a source grant, or picks a newer revision. */
export function useMobileReadingLinks(navigation: MutableRefObject<number>, handlers: Handlers) {
  const backend = useReaderBackend();
  const current = useRef(handlers);
  current.current = handlers;
  const mounted = useRef(true);
  const open = useCallback(async (reference: CanonicalReaderReference) => {
    const epoch = ++navigation.current;
    const selected = () => mounted.current && navigation.current === epoch;
    if (reference.documentId) {
      const reply = await backend.mobile.mobileReaderRead(reference.documentId, reference.revision);
      if (!selected()) return;
      if (reply.status === 'error') throw new Error('This exact imported copy is unavailable in your current library.');
      const document = reply.data.document;
      if (reference.path !== 'document.md' || document.id !== reference.documentId || document.source_id !== reference.sourceId || document.revision !== reference.revision)
        throw new Error('The exact device receipt is unavailable. No newer copy was opened.');
      await current.current.openDevice(document, reference.line ?? 1);
      return;
    }
    const exact = providerTarget({ sourceId: reference.sourceId, path: reference.path, revision: reference.revision,
      name: reference.title ?? reference.path, line: reference.line ?? 1 });
    if (!exact || !exact.sourceId.startsWith('provider:')) throw new Error('This mobile library supports imported and connected cloud document links.');
    const body = await nativeProvidersBackend.readDocument(exact).catch(cause => {
      if (selected()) throw cause;
      return null;
    });
    if (!selected() || !body) return;
    if (body.sourceId !== exact.sourceId || body.path !== exact.path || body.revision !== exact.revision)
      throw new Error('The exact cached cloud copy is unavailable. No newer copy was opened.');
    await current.current.openProvider(exact);
  }, [backend, navigation]);
  useEffect(() => {
    mounted.current = true;
    let active = true;
    let stop: (() => void) | undefined;
    void observeReadingLinks(open, message => { if (active) current.current.failed(message); })
      .then(dispose => { if (active) stop = dispose; else dispose(); }).catch(() => {});
    return () => { active = false; mounted.current = false; navigation.current++; stop?.(); };
  }, [open, navigation]);
  return open;
}
