import { useEffect, useRef, useState } from 'react';
import type { CanonicalReaderReference } from '../reader/contracts';
import type { ProviderDocumentBody } from '../../platform/providers';
import { readingLink } from './reading-link';
import { ActionIcon } from './ActionIcon';
import './document-transfer.css';

export function DocumentActions({ reference, read, line = reference.line ?? 1, getLine, nativeExport, nativeShare, includesSelectedImages = false, nativeReadingLinks = false }: {
  reference: CanonicalReaderReference;
  read(): Promise<ProviderDocumentBody>;
  line?: number;
  getLine?: () => number;
  /** Present only when a real native save-dialog transport is composed. */
  nativeExport?: (reference: CanonicalReaderReference) => Promise<'exported' | 'cancelled'>;
  nativeShare?: (reference: CanonicalReaderReference) => Promise<'presented' | 'cancelled'>;
  includesSelectedImages?: boolean;
  nativeReadingLinks?: boolean;
}) {
  const [prepared, setPrepared] = useState<{ reference: CanonicalReaderReference; file: File } | null>(null);
  const file = prepared?.reference === reference ? prepared.file : null;
  const [notice, setNotice] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [linkLine, setLinkLine] = useState(line);
  const [showLink, setShowLink] = useState(false);
  const epoch = useRef(0);
  useEffect(() => {
    epoch.current++; setPrepared(null); setNotice(null); setError(null); setBusy(false); setLinkLine(line); setShowLink(false);
    return () => { epoch.current++; };
  }, [reference]);
  const native = '__TAURI_INTERNALS__' in window;
  let link = '';
  try { link = readingLink(reference, linkLine); } catch { /* An invalid receipt exposes no link. */ }
  async function prepare() {
    const selected = epoch.current;
    setBusy(true); setError(null); setNotice(null);
    try {
      const body = await read();
      if (selected !== epoch.current) return;
      if (body.sourceId !== reference.sourceId || body.path !== reference.path || body.revision !== reference.revision)
        throw new Error('The exact document revision could not be verified.');
      if (new TextEncoder().encode(body.markdown).length > 2 * 1024 * 1024)
        throw new Error('This export exceeds the supported size. The original is unchanged.');
      const name = (reference.title || reference.path.split('/').at(-1) || 'document')
        .replace(/[\\/:*?"<>|]/g, '_').slice(0, 120).replace(/\.md$/i, '') + '.md';
      const copy = new File([body.markdown], name, { type: 'text/markdown;charset=utf-8' });
      setPrepared({ reference, file: copy });
      return copy;
    } catch (cause) { if (selected === epoch.current) setError(cause instanceof Error ? cause.message : 'The document could not be prepared.'); }
    finally { if (selected === epoch.current) setBusy(false); }
  }
  async function exportNative() {
    if (!nativeExport) return;
    const selected = epoch.current;
    setBusy(true); setError(null); setNotice(null);
    try {
      const state = await nativeExport(reference);
      if (selected === epoch.current) setNotice(state === 'exported' ? (includesSelectedImages ? 'Document copy exported to your selected destination.' : 'Markdown exported to your selected destination.') : 'Export cancelled.');
    } catch (cause) { if (selected === epoch.current) setError(cause instanceof Error ? cause.message : 'Native export is unavailable.'); }
    finally { if (selected === epoch.current) setBusy(false); }
  }
  async function shareNative() {
    if (!nativeShare) return;
    const selected = epoch.current;
    setBusy(true); setError(null); setNotice(null);
    try {
      const state = await nativeShare(reference);
      if (selected === epoch.current) setNotice(state === 'presented' ? 'Copy presented to the system share sheet. Check the receiving app to confirm delivery.' : 'Sharing cancelled.');
    } catch (cause) { if (selected === epoch.current) setError(cause instanceof Error ? cause.message : 'Sharing could not be confirmed.'); }
    finally { if (selected === epoch.current) setBusy(false); }
  }
  async function download() {
    if (native || busy) return;
    const selected = epoch.current;
    const copy = file ?? await prepare();
    if (!copy || selected !== epoch.current) return;
    const url = URL.createObjectURL(copy);
    const anchor = document.createElement('a');
    anchor.href = url; anchor.download = copy.name; anchor.click();
    window.setTimeout(() => URL.revokeObjectURL(url), 1000);
    setNotice('Markdown download requested. Check your browser downloads.');
  }
  async function share() {
    if (!file) {
      const copy = await prepare();
      if (copy) setNotice(navigator.canShare?.({ files: [copy] }) ? 'Your Markdown copy is ready. Choose Share again to open the share sheet.' : 'File sharing is unavailable in this browser. Use Export to save a copy.');
      return;
    }
    if (!navigator.canShare?.({ files: [file] })) return;
    const selected = epoch.current;
    setError(null); setNotice(null);
    try { await navigator.share({ files: [file], title: file.name }); if (selected === epoch.current) setNotice('Share sheet closed.'); }
    catch (cause) { if (selected === epoch.current) setError(cause instanceof Error && cause.name === 'AbortError' ? 'Sharing cancelled.' : 'Sharing is unavailable. The document is unchanged.'); }
  }
  async function copy() {
    if (!link) return;
    const selected = epoch.current;
    setError(null); setNotice(null);
    try {
      const currentLine = getLine?.() ?? line;
      const currentLink = readingLink(reference, currentLine);
      setLinkLine(currentLine);
      await navigator.clipboard.writeText(currentLink); if (selected === epoch.current) setNotice('Reading link copied.'); }
    catch { if (selected === epoch.current) { setShowLink(true); setError('Couldn’t copy the link. Select it below and copy it manually.'); } }
  }
  return <section className="document-actions" aria-label="Share and export" aria-busy={busy}>
    <div className="document-actions__buttons" role="group" aria-label="Export and link actions">
      {nativeExport || !native ? <button type="button" className="document-actions__export" disabled={busy}
        aria-label={includesSelectedImages ? 'Export document copy' : 'Export Markdown'}
        title={includesSelectedImages ? 'Save Markdown and selected images' : 'Save a Markdown copy'}
        onClick={() => void (nativeExport ? exportNative() : download())}><ActionIcon kind="export" />Export</button> : null}
      {nativeShare || !native && typeof navigator.share === 'function' ? <button type="button" disabled={busy}
        aria-label="Share document copy" onClick={() => void (nativeShare ? shareNative() : share())}><ActionIcon kind="share" />Share</button> : null}
      <button type="button" disabled={!link} aria-label="Copy reading link" onClick={() => void copy()}><ActionIcon kind="link" />Copy link</button>
      <button type="button" className="document-actions__details-toggle" aria-label="Reading link details"
        aria-expanded={showLink} onClick={() => { setShowLink(!showLink); setLinkLine(getLine?.() ?? line); }}>Link details</button>
    </div>
    {showLink ? <div className="document-actions__link-details">
      <label>Reading link<input readOnly value={link} onFocus={(event) => event.currentTarget.select()} /></label>
      <p>Opens this version at your current reading position in an already connected library. {nativeReadingLinks ? 'Tap the link on this device or use Open reading link in MDReader.' : 'Use Open reading link in MDReader.'} It doesn’t share the file or grant access.</p>
      <p>{includesSelectedImages ? 'Export includes Markdown and selected offline images in a ZIP.' : 'Export saves a Markdown copy.'} Your original stays unchanged.</p>
    </div> : null}
    {notice ? <p role="status">{notice}</p> : null}
    {error ? <p role="alert">{error}</p> : null}
  </section>;
}
