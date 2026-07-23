/**
 * The related-documents rail on the reader: a collapsible right column showing
 * `related()` of the active document, each row score-decomposed (graph /
 * concept / vector) in a tooltip. Fetches lazily — only while open, and
 * debounced so a rapid tab settle coalesces to one call — and is guarded by the
 * frontend open-epoch plus a per-request id so a workspace switch (or a fast
 * doc change) can never surface a previous document's related list.
 */

import { useEffect, useRef, useState } from 'react';
import type { RelatedHit, SearchAdapter } from '@mdreader/ui';

import type { DocRef } from '../stores/types.js';

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
  onOpen: (ref: DocRef) => void;
  /**
   * The active workspace key (its root). A change is a workspace switch: it
   * re-runs the fetch effect (dropping any in-flight result for the previous
   * workspace), the frontend half of the revocation barrier the backend
   * `related()` also enforces — so the rail never shows a stale workspace's list.
   */
  workspaceKey: string | null;
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
}: RelatedRailProps) {
  const [items, setItems] = useState<RelatedHit[]>([]);
  const [status, setStatus] = useState<RailStatus>('idle');
  const reqRef = useRef(0);

  const activePath = activeDoc?.path ?? null;

  useEffect(() => {
    if (!open || !activePath) {
      setItems([]);
      setStatus('idle');
      return;
    }
    const id = (reqRef.current += 1);
    setStatus('loading');
    const handle = window.setTimeout(() => {
      adapter
        .related(activePath)
        .then((hits) => {
          // Superseded by a newer fetch (doc change) or a workspace switch
          // (epoch is in the effect deps, so this closure is the current one).
          if (reqRef.current !== id) return;
          setItems(hits);
          setStatus('ready');
        })
        .catch(() => {
          if (reqRef.current !== id) return;
          setItems([]);
          setStatus('error');
        });
    }, SETTLE_MS);
    return () => window.clearTimeout(handle);
  }, [open, activePath, workspaceKey, adapter]);

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
        {!activePath ? (
          <p className="related-rail__empty">Open a document to see related notes.</p>
        ) : status === 'loading' ? (
          <p className="related-rail__empty">Finding related notes…</p>
        ) : status === 'error' ? (
          <p className="related-rail__empty">Related notes unavailable.</p>
        ) : items.length === 0 ? (
          <p className="related-rail__empty">No related notes yet.</p>
        ) : (
          <ul className="related-rail__list">
            {items.map((hit) => {
              const { graph, concept, vector } = hit.components;
              const tip = `score ${hit.score.toFixed(2)} · graph ${graph.toFixed(
                2,
              )} · concept ${concept.toFixed(2)} · vector ${vector.toFixed(2)}`;
              return (
                <li key={hit.documentId} className="related-rail__item">
                  <button
                    type="button"
                    className="related-rail__link"
                    title={tip}
                    onClick={() => onOpen({ path: hit.path, name: hit.name })}
                  >
                    <span className="related-rail__name">{hit.name}</span>
                    <span className="related-rail__score">{hit.score.toFixed(2)}</span>
                    <span className="related-rail__chips" aria-hidden="true">
                      {graph > 0 ? <i className="related-rail__chip related-rail__chip--graph" /> : null}
                      {concept > 0 ? (
                        <i className="related-rail__chip related-rail__chip--concept" />
                      ) : null}
                      {vector > 0 ? (
                        <i className="related-rail__chip related-rail__chip--vector" />
                      ) : null}
                    </span>
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
