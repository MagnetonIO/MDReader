import { useEffect, useRef, useState } from 'react';
import type { ComponentType, ReactNode } from 'react';
import { MarkdownReader, PlatformProvider } from '@mdreader/ui';
import type { MarkdownReaderHandle, NavigateEvent } from '@mdreader/ui';
import { parseMarkdownAuto } from '@mdreader/markdown/worker';
import { resolveHeading } from '@mdreader/markdown';
import type { ParsedDoc } from '@mdreader/markdown';
import {
  nativeProvidersBackend, type ProviderReaderReference,
  type ProviderLinkResolution, type ProviderLinkCandidate, type ProviderProjection,
} from '../../platform/providers';

import type { ReaderPosition } from '../reader-history/model';
import { savedAnchor, restoreAnchor } from '../reader-history/positions';
import { exportNativeMarkdown } from '../document-transfer/native-export';
import { DocumentActions } from '../document-transfer/DocumentActions';
import type { CanonicalReaderBackend, LocalLinkResolution, DeviceLinkResolution, CanonicalReaderReference } from './contracts';
import './canonical-reader.css';
import { LocalImageProvider, LocalSourceImage, useLocalImages } from './LocalSourceImage';
import { textScaleFactor, useSettingsStore } from '../../stores/settings';
import { readingAppearance } from './appearance';
import { readExact } from './exact-read';
import { openReaderExternal } from '../../platform/external-links';
import type { AssetPreview } from '../../bindings';
import { AttachmentLayer } from './AttachmentLayer';
import { isAttachmentLink } from './attachment-preview';
import { useAppearanceChange } from './useAppearanceChange';
import { DocumentDetails } from './DocumentDetails';

export interface ReaderPresentation {
  title: string;
  close(): void;
  previous?: () => void;
  forward?: () => void;
  outline: ParsedDoc['outline'];
  onOutline(line: number): void;
  onAppearanceChange(change: () => void): void;
  actions: ReactNode;
  details: ReactNode;
}

export function CanonicalReader({ reference, backend = nativeProvidersBackend, onClose, closeLabel = 'Back to sources', onVerifiedRead, sourceKind = 'provider', imageComponent: Image, position = null, onPosition, onNavigateReference, onBack, onForward, canGoBack = false, canGoForward = false, readingControls, onEdit, editBusy = false, documentActions = true, renderPresentation }: {
  reference: CanonicalReaderReference;
  sourceKind?: 'provider' | 'local' | 'device';
  imageComponent?: ComponentType<{ src?: string; alt?: string }>;
  backend?: CanonicalReaderBackend;
  onClose: () => void;
  closeLabel?: string;
  /** Lift only a verified current receipt; loading/failure clears availability. */
  onVerifiedRead?: (reference: CanonicalReaderReference, kind: 'local' | 'provider' | 'capture' | 'device' | null) => void;
  position?: ReaderPosition | null;
  onPosition?: (position: ReaderPosition) => void;
  onNavigateReference?: (reference: CanonicalReaderReference) => void;
  onBack?: () => void; onForward?: () => void;
  canGoBack?: boolean; canGoForward?: boolean;
  readingControls?: (scrollToLine: (line: number) => void) => ReactNode;
  onEdit?: (reference: ProviderReaderReference) => void;
  editBusy?: boolean;
  /** Hide transfer actions where this exact source kind has no native adapter. */
  documentActions?: boolean;
  /** Host chrome only; document ownership/navigation remain in this reader. */
  renderPresentation?: (controls: ReaderPresentation, document: ReactNode) => ReactNode;
}) {
  const readingFont = useSettingsStore((state) => state.readingFont);
  const textScaleIndex = useSettingsStore((state) => state.textScaleIndex);
  const reader = useRef<MarkdownReaderHandle | null>(null);
  const restored = useRef(false);
  const action = useRef(0);
  const [navigation, setNavigation] = useState({ origin: reference, current: reference, history: [] as CanonicalReaderReference[] });
  // A new parent selection takes effect immediately, including before effects.
  const currentReference = onNavigateReference ? reference : navigation.origin === reference ? navigation.current : reference;
  const selectedReference = useRef(currentReference);
  selectedReference.current = currentReference;
  const changeAppearance = useAppearanceChange(reader, currentReference);
  const verifiedRead = useRef(onVerifiedRead); verifiedRead.current = onVerifiedRead;
  const history = navigation.origin === reference ? navigation.history : [];
  const [loaded, setLoaded] = useState<{ reference: ProviderReaderReference; doc: ParsedDoc; sourceKind?: 'local' | 'provider' | 'capture'; line: number; anchor: { block: number; offset: number } | null } | null>(null);
  const doc = loaded?.reference === currentReference ? loaded.doc : null;
  const verifiedKind = loaded?.reference === currentReference ? loaded.sourceKind ?? sourceKind : sourceKind;
  const localImages = useLocalImages(currentReference, backend.readImage, Boolean(doc) && verifiedKind === 'local');
  const line = loaded?.reference === currentReference ? loaded.line : 1;
  const positionRef = useRef(position);
  positionRef.current = position;
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [attachment, setAttachment] = useState<{ origin: ProviderReaderReference; preview: AssetPreview } | null>(null);
  const currentAttachment = attachment?.origin === currentReference ? attachment.preview : null;
  const [links, setLinks] = useState<ProviderLinkResolution | LocalLinkResolution | DeviceLinkResolution | null>(null);
  const [busy, setBusy] = useState(false);
  const [detailsOpen, setDetailsOpen] = useState(false);
  const [readAttempt, setReadAttempt] = useState(false);
  const [retryReference, setRetryReference] = useState<CanonicalReaderReference | null>(null);
  const [waitingForOwner, setWaitingForOwner] = useState(false);
  const readInFlight = useRef(false);
  useEffect(() => {
    let current = true;
    const cancelled = new AbortController();
    verifiedRead.current?.(currentReference, null);
    readInFlight.current = true;
    action.current++;
    setLoaded(null);
    restored.current = false;
    setError(null); setNotice(null); setLinks(null); setAttachment(null); setBusy(false); setRetryReference(null); setWaitingForOwner(false);
    const savedPosition = positionRef.current;
    void readExact(() => selectedReference.current === currentReference
      ? backend.readDocument(currentReference) : Promise.reject(new DOMException('The reading selection changed.', 'AbortError')), cancelled.signal,
      sourceKind === 'device' ? backend.isReadRetryable : undefined,
      () => { if (current && selectedReference.current === currentReference) setWaitingForOwner(true); }).then(async (value) => {
      if (value.sourceId !== currentReference.sourceId || value.path !== currentReference.path || value.revision !== currentReference.revision)
        throw new Error('The selected document revision is no longer available.');
      const parsed = await parseMarkdownAuto(value.markdown);
      if (!current || selectedReference.current !== currentReference) return;
      let targetLine = currentReference.line ?? 1;
      if (currentReference.heading) {
        const heading = resolveHeading(parsed.outline, currentReference.heading);
        if (heading) targetLine = heading.lineStart;
      }
      const restoredPosition = restoreAnchor(parsed, currentReference.revision, value.lineEnd, currentReference.heading ? null : savedPosition);
      if (restoredPosition.notice) setNotice(restoredPosition.notice);
      setLoaded({ reference: currentReference, doc: parsed, sourceKind: value.sourceKind, line: restoredPosition.line ?? targetLine, anchor: restoredPosition.anchor });
      verifiedRead.current?.(currentReference, value.sourceKind ?? sourceKind);
    }).catch((cause: unknown) => {
      if (current && selectedReference.current === currentReference) {
        setError(cause instanceof Error ? cause.message : 'The document is unavailable.');
        setRetryReference(backend.isReadRetryable?.(cause) ? currentReference : null);
      }
    }).finally(() => {
      if (current && selectedReference.current === currentReference) { readInFlight.current = false; setWaitingForOwner(false); }
    });
    return () => { current = false; cancelled.abort(); action.current++; };
  }, [backend, currentReference, readAttempt, sourceKind]);
  function retryRead() {
    if (readInFlight.current || retryReference !== currentReference) return;
    readInFlight.current = true;
    setRetryReference(null); setError(null);
    setReadAttempt((previous) => !previous);
  }
  function close() { action.current++; onClose(); }
  function previous() {
    action.current++;
    if (onBack) { onBack(); return; }
    const target = history.at(-1);
    if (target) setNavigation({ origin: reference, current: target, history: history.slice(0, -1) });
  }
  function openLocal(next: ProviderReaderReference) {
    if (next.sourceId !== currentReference.sourceId) {
      throw new Error('The linked document source could not be verified.');
    }
    action.current++;
    if (onNavigateReference) onNavigateReference(next);
    else setNavigation({ origin: reference, current: next, history: [...history, currentReference].slice(-32) });
  }
  function openDevice(next: CanonicalReaderReference & { documentId: string }) {
    if (sourceKind !== 'device' || !onNavigateReference) {
      setError('Open this linked copy from Library to continue reading.');
      return;
    }
    action.current++;
    onNavigateReference(next);
  }
  function openProjection(projection: ProviderProjection, candidate: ProviderLinkCandidate, targetLine = 1, heading?: string | null) {
    if (projection.readerSourceId !== currentReference.sourceId || !Number.isSafeInteger(targetLine) || targetLine < 1)
      throw new Error('The linked document source could not be verified.');
    const next: ProviderReaderReference = { sourceId: projection.readerSourceId, path: projection.path, revision: projection.revision, title: candidate.title, line: targetLine };
    if (heading) next.heading = heading;
    if (onNavigateReference) onNavigateReference(next);
    else setNavigation({ origin: reference, current: next, history: [...history, currentReference].slice(-32) });
  }
  async function navigate(event: NavigateEvent) {
    if (event.kind === 'anchor') {
      action.current++;
      setBusy(false); setLinks(null); setAttachment(null); setError(null); setNotice(null);
      if (!reader.current?.scrollToFragment(event.slug)) setNotice('The requested location is unavailable in this revision.');
      return;
    }
    const epoch = ++action.current;
    const origin = currentReference;
    setError(null); setNotice(null); setLinks(null); setAttachment(null); setBusy(true);
    try {
      if (event.kind === 'relative' && backend.readAttachment && isAttachmentLink(event.href)) {
        const preview = await backend.readAttachment(origin, event.href);
        if (action.current === epoch && selectedReference.current === origin) setAttachment({ origin, preview });
        return;
      }
      if (!backend.resolveLink) throw new Error('Link resolution is unavailable in this reader.');
      const target = event.kind === 'relative' ? event.href : `${event.target}${event.heading ? `#${event.heading}` : ''}`;
      const resolution = await backend.resolveLink(currentReference, event.kind, target);
      if (action.current !== epoch || selectedReference.current !== origin) return;
      if (resolution.state === 'local_matches' && resolution.references.length === 1) {
        openLocal(resolution.references[0]!);
      } else if (resolution.state === 'device_matches' && resolution.references.length === 1) {
        openDevice(resolution.references[0]!);
      } else if (resolution.state === 'resolved') openProjection(resolution.projection, resolution.candidate, resolution.line, resolution.heading);
      else setLinks(resolution);
    } catch (cause) {
      if (action.current === epoch && selectedReference.current === origin) setError(cause instanceof Error ? cause.message : 'The linked document is unavailable.');
    } finally {
      if (action.current === epoch && selectedReference.current === origin) setBusy(false);
    }
  }
  async function choose(candidate: ProviderLinkCandidate, fetch: boolean, heading: string | null) {
    if (busy) return;
    const epoch = ++action.current;
    const origin = currentReference;
    setError(null); setNotice(null); setBusy(true);
    try {
      const operation = fetch ? backend.fetchCurrent : backend.readCached;
      if (!operation) throw new Error('This document operation is unavailable in this reader.');
      const projection = await operation(candidate.sourceId, candidate.documentId);
      if (action.current === epoch && selectedReference.current === origin) openProjection(projection, candidate, 1, heading);
    } catch (cause) {
      if (action.current === epoch && selectedReference.current === origin) setError(cause instanceof Error ? cause.message : 'The linked document is unavailable.');
    } finally {
      if (action.current === epoch && selectedReference.current === origin) setBusy(false);
    }
  }
  const details = <DocumentDetails reference={currentReference} kind={verifiedKind} verified={Boolean(doc)} />;
  const actions = <>
    {doc && readingControls ? readingControls((target) => reader.current?.scrollToLine(target)) : null}
    {doc && documentActions ? <DocumentActions reference={currentReference} read={() => backend.readDocument(currentReference)} nativeExport={'__TAURI_INTERNALS__' in window ? exportNativeMarkdown : undefined} line={position?.revision === currentReference.revision ? position.line : currentReference.line ?? 1} /> : null}
  </>;
  const documentContent = <>
    {error ? <p role="alert">{error}</p> : null}
    {retryReference === currentReference && error ? <div className="settings-account-actions">
      <button type="button" className="settings-link-button" onClick={retryRead}>Retry read</button>
    </div> : null}
    {notice ? <p role="status">{notice}</p> : null}
    {busy ? <p role="status">Opening linked document…</p> : null}
    {links?.state === 'local_matches' ? (
      <div className="provider-reader-dialog__choices">
        {links.references.length ? <>
          <p>This link matches more than one indexed document. Choose the exact copy.</p>
          {links.references.map((target) => <button
            key={`${target.sourceId}:${target.path}`}
            type="button"
            onClick={() => openLocal(target)}
          >Read exact {target.path}</button>)}
        </> : <p role="status">{links.linkKind === 'wikilink' ? 'No indexed note matches this name in this source.' : 'The linked file could not be found in this folder.'}</p>}
      </div>
    ) : null}
    {links?.state === 'device_matches' ? <div className="provider-reader-dialog__choices">
      {links.references.length ? <>
        <p>Choose the imported copy to read.</p>
        {links.references.map((target) => <button key={`${target.documentId}:${target.revision}`}
          type="button" disabled={busy} onClick={() => openDevice(target)}>{target.title || 'Imported document'}</button>)}
      </> : <p role="status">This linked document was not included in the import, or its exact copy is unavailable.</p>}
    </div> : null}
    {links?.state === 'missing' ? <p role="status">No matching document is available in the current selected metadata. Refresh or select it in Sources.</p> : null}
    {links?.state === 'selected_uncached' ? <div className="provider-reader-dialog__choices">
      <p>This selected document is not cached. Fetch it explicitly to read it.</p>
      <button type="button" disabled={busy} onClick={() => void choose(links.candidate, true, links.heading)}>Fetch current {links.candidate.title}</button>
    </div> : null}
    {links?.state === 'ambiguous' ? <div className="provider-reader-dialog__choices">
      <p>This link matches more than one selected document. Choose a copy to read.</p>
      {links.candidates.map((candidate, index) => <div key={`${candidate.sourceId}/${candidate.documentId}`}>
        <span>{candidate.title} (choice {index + 1})</span>
        <button type="button" disabled={busy} onClick={() => void choose(candidate, false, links.heading)}>Read cached {candidate.title} (choice {index + 1})</button>
        <button type="button" disabled={busy} onClick={() => void choose(candidate, true, links.heading)}>Fetch current {candidate.title} (choice {index + 1})</button>
      </div>)}
    </div> : null}
    {links?.state === 'heading_missing' ? <div className="provider-reader-dialog__choices">
      <p>The requested heading is unavailable in the cached revision.</p>
      <button type="button" disabled={busy} onClick={() => void choose(links.candidate, false, null)}>Read cached {links.candidate.title} from beginning</button>
    </div> : null}
    <AttachmentLayer origin={currentReference} preview={currentAttachment} onClose={() => { action.current++; setAttachment(null); }}>
    <div className="provider-reader-dialog__document" style={readingAppearance(readingFont, textScaleFactor({ textScaleIndex }))}>
      {doc ? <LocalImageProvider images={localImages}><PlatformProvider platform={{ resolveAssetUrl: () => 'about:blank', openExternal(url) {
        void openReaderExternal(url).catch((cause: unknown) => {
          if (selectedReference.current === currentReference) setError(cause instanceof Error ? cause.message : 'The link could not be opened.');
        });
      } }}>
        <MarkdownReader
          ref={(handle) => {
            reader.current = handle;
            if (handle && !restored.current) {
              if (currentReference.heading) {
                const heading = resolveHeading(doc.outline, currentReference.heading);
                if (!handle.scrollToFragment(currentReference.heading)) {
                  if (heading) handle.scrollToLine(heading.lineStart);
                  else { handle.scrollToLine(1); setNotice('The requested location is unavailable in this revision. Showing the beginning of the document.'); }
                }
              }
              else if (loaded?.anchor) handle.scrollToBlock(loaded.anchor.block, loaded.anchor.offset);
              else handle.scrollToLine(line);
              restored.current = true;
            }
          }}
          doc={doc}
          onScrollAnchorChange={onPosition ? (anchor) => {
            if (loaded?.reference !== currentReference) return;
            const position = savedAnchor(doc, currentReference.revision, anchor);
            if (position) onPosition(position);
          } : undefined}
          components={{ img: Image ?? (verifiedKind === 'local' && backend.readImage ? LocalSourceImage : (({ alt }) => <span>{alt || 'Image'} — asset is not available in this copy.</span>)) }}
          onNavigate={(event) => void navigate(event)} />
      </PlatformProvider></LocalImageProvider> : !error ? <p role="status">{waitingForOwner ? 'Library is busy. Waiting to read this exact copy…' : 'Reading Markdown…'}</p> : null}
    </div>
    </AttachmentLayer>
  </>;
  return <section className="canonical-reader" aria-label={sourceKind === 'device' ? 'Device document' : sourceKind === 'local' ? 'Catalog document' : 'Cloud document'}>
    {renderPresentation ? renderPresentation({
      title: currentReference.title || currentReference.path, close,
      previous: (onBack ? canGoBack : history.length > 0) ? previous : undefined,
      forward: canGoForward && onForward ? onForward : undefined,
      outline: doc?.outline ?? [], onOutline: target => reader.current?.scrollToLine(target),
      onAppearanceChange: changeAppearance, actions, details,
    }, documentContent) : <>
    <header className="provider-reader-dialog__bar">
      <button type="button" autoFocus onClick={close}>{closeLabel}</button>
      {(onBack ? canGoBack : history.length > 0) ? <button type="button" onClick={previous}>Previous document</button> : null}
      {canGoForward && onForward ? <button type="button" onClick={onForward}>Next document</button> : null}
      <strong>{currentReference.title || currentReference.path}</strong>
      {!onEdit || verifiedKind === 'capture' ? <span>Read only</span> : null}
      {onEdit && verifiedKind !== 'capture' ? <button type="button" disabled={!doc || editBusy}
        onClick={() => onEdit(currentReference)}>{editBusy ? 'Opening editor…' : sourceKind === 'local' ? 'Edit local file' : sourceKind === 'device' ? 'Edit Markdown' : 'Edit cloud document'}</button> : null}
    </header>
    <div className="canonical-reader__tools" role="group" aria-label="Document actions">
      {actions}
      <details className="provider-reader-dialog__evidence" onToggle={event => setDetailsOpen(event.currentTarget.open)}><summary>Document details</summary>{detailsOpen ? details : null}</details>
    </div>
    {documentContent}
    </>}
  </section>;
}
