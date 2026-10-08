import { useEffect, useRef, useState } from 'react';
import type { CanonicalReaderReference } from '../reader/contracts';
import type { ProviderDocumentBody } from '../../platform/providers';
import { readingLink } from './reading-link';
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
  const epoch = useRef(0);
  useEffect(() => {
    epoch.current++; setPrepared(null); setNotice(null); setError(null); setBusy(false); setLinkLine(line);
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
      setPrepared({ reference, file: new File([body.markdown], name, { type: 'text/markdown;charset=utf-8' }) });
      setNotice('Exact Markdown prepared. Selected images are separate and are not included.');
    } catch (cause) { if (selected === epoch.current) setError(cause instanceof Error ? cause.message : 'The document could not be prepared.'); }
    finally { if (selected === epoch.current) setBusy(false); }
  }
  async function exportNative() {
    if (!nativeExport) return;
    const selected = epoch.current;
    setBusy(true); setError(null);
    try {
      const state = await nativeExport(reference);
      if (selected === epoch.current) setNotice(state === 'exported' ? (includesSelectedImages ? 'Document copy exported with selected offline images.' : 'Markdown exported to your selected destination.') : 'Export cancelled.');
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
  function download() {
    if (!file || native) return;
    const url = URL.createObjectURL(file);
    const anchor = document.createElement('a');
    anchor.href = url; anchor.download = file.name; anchor.click();
    window.setTimeout(() => URL.revokeObjectURL(url), 1000);
    setNotice('Markdown download requested. Check your browser downloads.');
  }
  async function share() {
    if (!file || !navigator.canShare?.({ files: [file] })) return;
    const selected = epoch.current;
    try { await navigator.share({ files: [file], title: file.name }); if (selected === epoch.current) setNotice('Share sheet closed.'); }
    catch (cause) { if (selected === epoch.current) setError(cause instanceof Error && cause.name === 'AbortError' ? 'Sharing cancelled.' : 'Sharing is unavailable. The document is unchanged.'); }
  }
  async function copy() {
    if (!link) return;
    const selected = epoch.current;
    try {
      const currentLine = getLine?.() ?? line;
      const currentLink = readingLink(reference, currentLine);
      setLinkLine(currentLine);
      await navigator.clipboard.writeText(currentLink); if (selected === epoch.current) setNotice('Reading link copied. Paste it into MDReader in the same authorized library.'); }
    catch { if (selected === epoch.current) setError('Clipboard access is unavailable. Select and copy the reading link below.'); }
  }
  return <details className="document-actions" onToggle={(event) => { if (event.currentTarget.open) setLinkLine(getLine?.() ?? line); }}>
    <summary>Export, share &amp; reading link</summary>
    <p>{includesSelectedImages ? "Original Markdown and selected offline images. Copies with images use a ZIP; incomplete and unselected images are excluded." : "Original Markdown only."} Exporting a copy never changes the source.</p>
    <div className="document-actions__buttons">
      {nativeExport ? <button type="button" disabled={busy} onClick={() => void exportNative()}>{includesSelectedImages ? "Export document copy…" : "Export Markdown…"}</button> : native ? <p>Native file export is unavailable in this build.</p> : null}
      {nativeShare ? <button type="button" disabled={busy} onClick={() => void shareNative()}>Share document copy…</button> : null}
      {!native || !nativeShare && typeof navigator.share === 'function' ? <button type="button" disabled={busy} onClick={() => void prepare()}>Prepare Markdown copy</button> : null}
      {!native && file ? <button type="button" onClick={download}>Download Markdown</button> : null}
      {file && navigator.canShare?.({ files: [file] }) ? <button type="button" onClick={() => void share()}>Share Markdown…</button> : null}
      {!nativeShare && typeof navigator.share !== 'function' ? <p>System sharing is unavailable on this host.</p> : null}
      <button type="button" disabled={!link} onClick={() => void copy()}>Copy reading link</button>
    </div>
    <label>Exact reading link<input readOnly value={link} onFocus={(event) => event.currentTarget.select()} /></label>
    <p>This link works only with an existing authorized source and exact revision. {nativeReadingLinks ? "Tap it on this device, or paste it into Open reading link." : "Paste it into Open reading link; opening links from another app is unavailable on this host."}</p>
    {notice ? <p role="status">{notice}</p> : null}
    {error ? <p role="alert">{error}</p> : null}
  </details>;
}
