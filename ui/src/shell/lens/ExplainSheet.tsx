/**
 * **Query Explanation**: why this result, at this position.
 *
 * A hybrid ranker is a stack of decisions, and an unexplained score is just a
 * number to be trusted. This shows the whole derivation instead: which arms
 * matched and at what rank, the reciprocal-rank fusion of those ranks, each
 * boost's recorded contribution, the final ordering key, the index version and
 * embedding model the semantic arm actually searched, and the
 * chunk → section → document → file → source chain the hit came out of.
 *
 * A boost that never fired reads "did not fire", not `0.0000` — a contribution
 * of zero and an absent contribution are different claims about the ranker, and
 * only one of them is true here. Likewise a hit found by one arm says which arm
 * missed, rather than showing an empty rank column.
 */

import { useCallback, useState } from 'react';

import type { ExplainedHit, HitBoosts, SemanticsAdapter } from '../../platform/semantics.js';
import { armsSentence, boostLabel, indexLabel, rankLabel, scoreLabel, shortId } from './lens-format.js';

export interface ExplainSheetProps {
  adapter: SemanticsAdapter;
  /** Seed query (e.g. carried over from the search palette). */
  initialQuery?: string;
  /** Open a hit at its section's first line. */
  onOpen: (hit: ExplainedHit) => void;
  onClose: () => void;
}

type Status = 'idle' | 'running' | 'ready' | 'error';

const BOOST_LABEL: Record<keyof HitBoosts, string> = {
  title: 'Title match',
  path: 'Path match',
  recency: 'Recency',
  graph: 'Graph proximity',
};

export function ExplainSheet({ adapter, initialQuery = '', onOpen, onClose }: ExplainSheetProps) {
  const [query, setQuery] = useState(initialQuery);
  const [hits, setHits] = useState<ExplainedHit[]>([]);
  const [status, setStatus] = useState<Status>('idle');
  const [error, setError] = useState<string | null>(null);

  const run = useCallback(async () => {
    const text = query.trim();
    if (text.length === 0) return;
    setStatus('running');
    try {
      const found = await adapter.explainQuery(text);
      setHits(found);
      setError(null);
      setStatus('ready');
    } catch (cause) {
      setHits([]);
      setError(cause instanceof Error ? cause.message : String(cause));
      setStatus('error');
    }
  }, [adapter, query]);

  return (
    <div
      className="overlay-backdrop"
      role="presentation"
      onMouseDown={onClose}
      onKeyDown={(event) => {
        if (event.key === 'Escape') onClose();
      }}
    >
      <div
        className="lens-sheet lens-sheet--explain"
        role="dialog"
        aria-label="Query explanation"
        onMouseDown={(event) => event.stopPropagation()}
      >
        <header className="lens-sheet__head">
          <h2 className="lens-sheet__title">Why these results</h2>
          <p className="lens-sheet__lede">
            Run a search and read its ranking back: which arms matched, how their ranks were fused,
            what each boost contributed, and where every hit came from.
          </p>
        </header>

        <form
          className="explain__form"
          onSubmit={(event) => {
            event.preventDefault();
            void run();
          }}
        >
          <input
            className="explain__input"
            type="search"
            value={query}
            placeholder="Search the corpus…"
            aria-label="Query to explain"
            onChange={(event) => setQuery(event.target.value)}
          />
          <button type="submit" className="btn-quiet" disabled={query.trim().length === 0}>
            Explain
          </button>
        </form>

        {status === 'error' ? (
          <p className="lens-sheet__empty">Could not run the query — {error}</p>
        ) : status === 'running' ? (
          <p className="lens-sheet__empty">Running both arms…</p>
        ) : status === 'idle' ? (
          <p className="lens-sheet__empty">Enter a query to see how it would be ranked.</p>
        ) : hits.length === 0 ? (
          <p className="lens-sheet__empty">Nothing matched. There is no ranking to explain.</p>
        ) : (
          <ol className="explains">
            {hits.map((hit, index) => (
              <ExplainRow key={hit.chunkId} hit={hit} position={index + 1} onOpen={onOpen} />
            ))}
          </ol>
        )}
      </div>
    </div>
  );
}

function ExplainRow({
  hit,
  position,
  onOpen,
}: {
  hit: ExplainedHit;
  position: number;
  onOpen: (hit: ExplainedHit) => void;
}) {
  const explain = hit.explain;
  return (
    <li className="explain">
      <div className="explain__head">
        <span className="explain__rank lens-mono">{position}</span>
        <button type="button" className="explain__title" onClick={() => onOpen(hit)}>
          {hit.name}
        </button>
        <span className="explain__score lens-mono">{scoreLabel(explain?.finalScore ?? hit.score)}</span>
      </div>

      {/* The semantic path: where inside the document the matched chunk sits. */}
      <div className="explain__path lens-mono">
        {hit.headingPath.length > 0 ? hit.headingPath.join(' › ') : 'the document body'}
        <span className="explain__lines">
          {' '}
          · lines {hit.startLine}
          {hit.endLine === hit.startLine ? '' : `–${hit.endLine}`}
        </span>
      </div>

      {explain === null ? (
        <p className="explain__none">No breakdown was recorded for this hit.</p>
      ) : (
        <>
          <p className="explain__arms">{armsSentence(explain)}</p>
          <dl className="explain__ranks">
            <div className="explain__ranks-row">
              <dt>Arms</dt>
              <dd className="lens-mono">
                {rankLabel(explain.lexRank, 'lexical')} · {rankLabel(explain.semRank, 'semantic')}
              </dd>
            </div>
            <div className="explain__ranks-row">
              <dt>RRF</dt>
              <dd className="lens-mono">{scoreLabel(explain.rrfScore)}</dd>
            </div>
          </dl>

          <ul className="boosts">
            {(Object.keys(BOOST_LABEL) as (keyof HitBoosts)[]).map((key) => {
              const contribution = explain.boosts[key];
              return (
                <li key={key} className={`boost${contribution === null ? ' boost--absent' : ''}`}>
                  <span className="boost__name">{BOOST_LABEL[key]}</span>
                  <span className="boost__bar" aria-hidden="true">
                    <i style={{ width: barWidth(contribution, explain.boosts) }} />
                  </span>
                  <span className="boost__value lens-mono">{boostLabel(contribution)}</span>
                </li>
              );
            })}
          </ul>

          <div className="explain__final lens-mono">
            RRF + boosts = <strong>{scoreLabel(explain.finalScore)}</strong>
          </div>

          <div className="explain__index">
            Semantic index: <span className="lens-mono">{indexLabel(explain)}</span>
          </div>

          {/* The full chain, ids and all: nothing about a hit is unattributable. */}
          <div className="chain lens-mono" aria-label="Provenance chain">
            <span title={explain.provenance.chunkId}>chunk {shortId(explain.provenance.chunkId)}</span>
            <span className="chain__to">→</span>
            <span title={explain.provenance.sectionId}>section {shortId(explain.provenance.sectionId)}</span>
            <span className="chain__to">→</span>
            <span title={explain.provenance.documentId}>document {shortId(explain.provenance.documentId)}</span>
            <span className="chain__to">→</span>
            <span title={explain.provenance.filePath}>{explain.provenance.filePath}</span>
            <span className="chain__to">→</span>
            <span title={explain.provenance.sourceRoot ?? undefined}>
              {explain.provenance.sourceId === null
                ? 'no attributing source'
                : `source ${shortId(explain.provenance.sourceId)}`}
            </span>
          </div>
        </>
      )}
    </li>
  );
}

/**
 * A boost bar, scaled against the largest contribution this hit recorded. An
 * absent contribution gets no bar at all — a zero-width bar and "did not fire"
 * would say two different things about the same absence.
 */
function barWidth(contribution: number | null, boosts: HitBoosts): string {
  if (contribution === null) return '0%';
  const magnitudes = Object.values(boosts)
    .filter((value): value is number => value !== null)
    .map(Math.abs);
  const peak = Math.max(...magnitudes, 0);
  if (peak === 0) return '0%';
  return `${Math.round((Math.abs(contribution) / peak) * 100)}%`;
}
