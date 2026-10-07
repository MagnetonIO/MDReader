import { useEffect, useRef, useState } from 'react';
import { MarkdownReader, PlatformProvider } from '@mdreader/ui';
import { parseMarkdownAuto } from '@mdreader/markdown/worker';
import type { ParsedDoc } from '@mdreader/markdown';
import { nativeProvidersBackend, type NativeProvidersBackend, type ProviderReaderReference } from '../platform/providers';

export function ProviderReaderDialog({ reference, backend = nativeProvidersBackend, onClose }: {
  reference: ProviderReaderReference;
  backend?: Pick<NativeProvidersBackend, 'readDocument'>;
  onClose: () => void;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const restored = useRef(false);
  const [doc, setDoc] = useState<ParsedDoc | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    const element = dialog.current;
    if (element?.showModal) element.showModal();
    else element?.setAttribute('open', '');
  }, []);
  useEffect(() => {
    let current = true;
    setDoc(null);
    restored.current = false;
    setError(null);
    void backend.readDocument(reference).then(async (value) => {
      if (value.sourceId !== reference.sourceId || value.path !== reference.path || value.revision !== reference.revision)
        throw new Error('The selected document revision is no longer available.');
      const parsed = await parseMarkdownAuto(value.markdown);
      if (current) setDoc(parsed);
    }).catch((cause: unknown) => {
      if (current) setError(cause instanceof Error ? cause.message : 'The document is unavailable.');
    });
    return () => { current = false; };
  }, [backend, reference]);
  return <dialog ref={dialog} className="provider-reader-dialog" aria-label="Cloud document" onCancel={(event) => { event.preventDefault(); onClose(); }}>
    <header className="provider-reader-dialog__bar">
      <button type="button" autoFocus onClick={onClose}>Back to sources</button>
      <strong>{reference.title || reference.path}</strong>
      <span>Read only</span>
    </header>
    <details className="provider-reader-dialog__evidence">
      <summary>Source evidence</summary>
      <p>{reference.sourceId}</p><p>{reference.path}</p><p>Revision {reference.revision}</p>
      <p>This is the exact locally admitted revision. Fetch current in Sources to check for a newer provider revision.</p>
    </details>
    {error ? <p role="alert">{error}</p> : null}
    <div className="provider-reader-dialog__document">
      {doc ? <PlatformProvider platform={{ resolveAssetUrl: () => 'about:blank', openExternal(url) { if (/^https:/i.test(url)) window.open(url, '_blank', 'noopener,noreferrer'); } }}>
        <MarkdownReader ref={(handle) => { if (handle && !restored.current) { handle.scrollToLine(reference.line ?? 1); restored.current = true; } }} doc={doc}
          components={{ img: ({ alt }) => <span>{alt || 'Image'} — asset is not available in this copy.</span> }}
          onNavigate={() => setError('Open linked documents from their selected cloud source.')} />
      </PlatformProvider> : !error ? <p role="status">Reading Markdown…</p> : null}
    </div>
  </dialog>;
}
