/**
 * The related-documents rail on the reader: a collapsible right column showing
 * `related()` of the active document, with plain link/tag/content reasons.
 * Fetches lazily — only while open, and
 * debounced so a rapid tab settle coalesces to one call — and is guarded by the
 * frontend open-epoch plus a per-request id so a workspace switch (or a fast
 * doc change) can never surface a previous document's related list.
 */

import { useEffect, useRef, useState } from 'react';
import { type HitTarget, relatedHits, relatedAvailabilityMessage, relatedReasons, type RelatedHit, type SearchAdapter } from '@mdreader/ui';


export interface RelatedRailProps {
  /** The open document (absolute path + name), or null when none is open. */
  activeDoc: { path: string; name: string } | null;
  /** The search transport (its `related` is used). */
  adapter: SearchAdapter;
  /** Whether the rail is expanded. */
  open: boolean;
  /** Toggle expanded/collapsed. */
  onToggle: () => void;
  /** Open a related document in a tab. */
  onOpen: (ref: HitTarget) => void;
  /**
   * The active workspace key (its root). A change is a workspace switch: it
   * re-runs the fetch effect (dropping any in-flight result for the previous
   * workspace), the frontend half of the revocation barrier the backend
   * `related()` also enforces — so the rail never shows a stale workspace's list.
   */
  workspaceKey: string | null;
  canonicalReference?: { sourceId: string; path: string; revision: string; deviceDocumentId?: string };
  refreshKey?: number | string;
}

type RailStatus = 'idle' | 'loading' | 'ready' | 'error';

/** Debounce before fetching, so a rapid tab settle coalesces to one call. */
const SETTLE_MS = 250;

export function RelatedRail({
  activeDoc,
  adapter,
  open,
  onToggle,
  onOpen,
  workspaceKey,
  canonicalReference,
  refreshKey = 0,
}: RelatedRailProps) {
  const [items, setItems] = useState<RelatedHit[]>([]);
  const [notice, setNotice] = useState<string | null>(null);
  const [status, setStatus] = useState<RailStatus>('idle');
  const reqRef = useRef(0);
  const [retry, setRetry] = useState(0);

  const activePath = activeDoc?.path ?? null;

  useEffect(() => {
    const id = (reqRef.current += 1);
    setItems([]);
    setNotice(null);
    if (!open || !activePath) {
      setItems([]);
      setStatus('idle');
      return;
    }
    setStatus('loading');
    const handle = window.setTimeout(() => {
      const request = canonicalReference
        ? adapter.relatedReference?.(canonicalReference) ?? Promise.reject(new Error('Related unavailable'))
        : adapter.related(activePath);
      request
        .then((hits) => {
          // Superseded by a newer fetch (doc change) or a workspace switch
          // (epoch is in the effect deps, so this closure is the current one).
          if (reqRef.current !== id) return;
          setItems(relatedHits(hits));
          setNotice(relatedAvailabilityMessage(hits));
          setStatus('ready');
        })
        .catch(() => {
          if (reqRef.current !== id) return;
          setItems([]);
          setStatus('error');
        });
    }, SETTLE_MS);
    return () => {
      window.clearTimeout(handle);
      reqRef.current += 1;
    };
  }, [open, activePath, workspaceKey, adapter, canonicalReference?.sourceId, canonicalReference?.revision, canonicalReference?.deviceDocumentId, refreshKey, retry]);

  if (!open) {
    return (
      <button
        type="button"
        className="related-rail__toggle related-rail__toggle--collapsed"
        onClick={onToggle}
        title="Show related notes"
        aria-expanded={false}
      >
        <span className="related-rail__toggle-label">Related</span>
      </button>
    );
  }

  return (
    <aside className="related-rail" aria-label="Related notes">
      <div className="related-rail__head">
        <span className="related-rail__title">Related</span>
        <button
          type="button"
          className="related-rail__toggle"
          onClick={onToggle}
          title="Hide related notes"
          aria-expanded
        >
          ×
        </button>
      </div>
      <div className="related-rail__body">
        {notice ? <p role="status" className="related-rail__empty">{notice}</p> : null}
        {!activePath ? (
          <p className="related-rail__empty">Open a document to see related notes.</p>
        ) : status === 'loading' ? (
          <p className="related-rail__empty">Finding related notes…</p>
        ) : status === 'error' ? (
          <div className="related-rail__empty"><p>Related notes could not be loaded.</p><button type="button" className="related-rail__retry" onClick={() => setRetry(value => value + 1)}>Retry</button></div>
        ) : items.length === 0 && !notice ? (
          <p className="related-rail__empty">No related notes yet.</p>
        ) : items.length === 0 ? null : (
          <ul className="related-rail__list">
            {items.map((hit) => {
              const reasons = relatedReasons(hit);
              return (
                <li key={hit.documentId} className="related-rail__item">
                  <button
                    type="button"
                    className="related-rail__link"
                    title={[hit.filePath, ...reasons].join(' · ')}
                    onClick={() => onOpen({ path: hit.path, name: hit.name, ...(hit.sourceId != null ? { sourceId: hit.sourceId } : {}), ...(hit.deviceDocumentId != null ? { deviceDocumentId: hit.deviceDocumentId } : {}), ...(hit.sourceKind != null ? { sourceKind: hit.sourceKind } : {}), ...(hit.revision != null ? { revision: hit.revision } : {}), ...(hit.line != null ? { line: hit.line } : {}) })}
                  >
                    <span className="related-rail__name">{hit.name}</span>
                    <span className="related-rail__reasons">{reasons.join(' · ')}</span>
                  </button>
                </li>
              );
            })}
          </ul>
        )}
      </div>
    </aside>
  );
}
