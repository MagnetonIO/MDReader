import { useEffect, useMemo, useRef, useState } from 'react';
import { CanonicalReader } from '../features/reader/CanonicalReader';
import { DocumentActions } from '../features/document-transfer/DocumentActions';
import { nativeProvidersBackend, type ProviderReaderReference } from '../platform/providers';
import type { CanonicalReaderBackend, CanonicalReaderReference } from '../features/reader/contracts';
import { providerTarget } from '../shell/provider-reading';
import { ProviderReadingControls } from './ProviderReadingControls';
import { ProviderDocumentActionError, providerDocumentActionsBackend, type ProviderDocumentActionsBackend } from './provider-document-actions';
import { useProviderReadingState } from './useProviderReadingState';
import type { TransferCapabilities } from './document-transfer';

/** Exact provider navigation remains controlled so actions always name the visible receipt. */
export function MobileProviderReader({ reference, onClose, closeLabel = 'Back to sources', readerBackend = nativeProvidersBackend, actionsBackend = providerDocumentActionsBackend }: {
  reference: ProviderReaderReference; onClose(): void; closeLabel?: string; readerBackend?: CanonicalReaderBackend; actionsBackend?: ProviderDocumentActionsBackend;
}) {
  const [navigation, setNavigation] = useState({ origin: reference, reference, history: [] as ProviderReaderReference[] });
  const current = navigation.origin === reference ? navigation.reference : reference;
  const history = navigation.origin === reference ? navigation.history : [];
  function navigate(next: CanonicalReaderReference) {
    if (next.documentId || next.sourceId !== reference.sourceId) throw new Error('The linked provider source could not be verified.');
    const verified = providerTarget({ sourceId: next.sourceId, path: next.path, revision: next.revision, name: next.title ?? next.path, line: next.line ?? 1 });
    if (!verified || !verified.sourceId.startsWith('provider:')) throw new Error('The exact linked provider receipt is unavailable.');
    if (next.heading) verified.heading = next.heading;
    setNavigation({ origin: reference, reference: verified, history: [...history, current].slice(-32) });
  }
  return <MobileProviderDocument key={`${current.sourceId}:${current.path}:${current.revision}:${current.line}:${current.heading}`} reference={current}
    onClose={onClose} closeLabel={closeLabel} readerBackend={readerBackend} actionsBackend={actionsBackend} onNavigate={navigate}
    canGoBack={history.length > 0} onBack={() => setNavigation({ origin: reference, reference: history.at(-1) ?? current, history: history.slice(0, -1) })} />;
}
function MobileProviderDocument({ reference, onClose, closeLabel, readerBackend, actionsBackend, onNavigate, canGoBack, onBack }: {
  reference: ProviderReaderReference; onClose(): void; closeLabel: string; readerBackend: CanonicalReaderBackend; actionsBackend: ProviderDocumentActionsBackend;
  onNavigate(reference: CanonicalReaderReference): void; canGoBack: boolean; onBack(): void;
}) {
  const reading = useProviderReadingState(reference, actionsBackend);
  const [line, setLine] = useState(reference.line ?? 1);
  const [support, setSupport] = useState<TransferCapabilities>({ export: false, share: false, readingLinks: false });
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    void actionsBackend.capabilities().then(value => { if (mounted.current) setSupport(value); }).catch(() => {});
    return () => { mounted.current = false; };
  }, [actionsBackend]);
  useEffect(() => { if (!reading.initializing) setLine(reading.initialLine); }, [reading.initializing, reading.initialLine]);
  const selected = useMemo(() => ({ ...reference, line: reading.initialLine }), [reference, reading.initialLine]);
  async function transfer(exact: CanonicalReaderReference, action: 'export' | 'share') {
    try { return await actionsBackend.transfer(exact, action); }
    catch (cause) {
      if (mounted.current && cause instanceof ProviderDocumentActionError && cause.kind === 'stale') reading.retire(cause);
      throw cause;
    }
  }
  if (reading.initializing) return <section aria-label="Cloud document"><button type="button" onClick={onClose}>{closeLabel}</button><p role="status">Reading your saved place…</p></section>;
  if (reading.unavailable) return <section aria-label="Cloud document"><button type="button" autoFocus onClick={onClose}>{closeLabel}</button><p role="alert">{reading.error}</p></section>;
  return <CanonicalReader reference={selected} sourceKind="provider" backend={readerBackend} documentActions={false} onClose={onClose} closeLabel={closeLabel}
    onNavigateReference={onNavigate} onBack={onBack} canGoBack={canGoBack}
    onPosition={position => { if (position.revision === reference.revision) setLine(position.line); }}
    readingControls={scrollToLine => <>
      <ProviderReadingControls reading={reading} line={line} onResume={value => { setLine(value); scrollToLine(value); }} />
      <DocumentActions reference={selected} read={() => readerBackend.readDocument(selected)}
        nativeExport={support.export ? async exact => { const result = await transfer(exact, 'export'); if (result === 'presented') throw new Error('Export could not be confirmed.'); return result; } : undefined}
        nativeShare={support.share ? async exact => { const result = await transfer(exact, 'share'); if (result === 'exported') throw new Error('Sharing could not be confirmed.'); return result; } : undefined} />
    </>} />;
}
