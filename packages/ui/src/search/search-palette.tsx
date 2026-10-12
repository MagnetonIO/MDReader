'use client';

/**
 * The Cmd+K search palette — and, in `file` mode, the Cmd+P jump (one component,
 * two modes, per the P4 unification). cmdk supplies accessible listbox/option
 * semantics and arrow/Enter navigation; search-specific behavior —
 * intents, the typed-filter passthrough with schema-derived errors inline, the
 * two-phase streamed merge, the provenance breadcrumb — is layered on top.
 *
 * Transport-agnostic: it drives a {@link SearchAdapter} (Tauri IPC in the
 * desktop app, HTTP on the website in P5) and reports opens through callbacks.
 * It never imports a platform.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Command } from 'cmdk';
import { SearchCoveragePanel } from './coverage-panel.js';
import { useSearchCoverage } from './use-search-coverage.js';

import { rankFiles, type PaletteFile } from './fuzzy.js';
import {
  SearchScopeMissingError,
  INTENTS,
  buildBreadcrumb,
  filterRelated,
  relatedHits as unwrapRelated,
  relatedAvailabilityMessage,
  relatedReasons,
  intentPlan,
  parseSnippet,
  sortHits,
  type Crumb,
  type RelatedHit,
  type SearchAdapter,
  type SearchHit,
  type SearchIntent,
} from './model.js';

export type { PaletteFile } from './fuzzy.js';
export type {
  RelatedHit,
  SearchAdapter,
  SearchExplain,
  SearchHit,
  SearchIntent,
  SearchOutcome,
} from './model.js';

/** Which mode the palette opens in: Cmd+K search, or the Cmd+P file jump. */
export type PaletteMode = 'search' | 'file';

/** A resolved open target: the document path, its name, and a provenance range. */
export interface HitTarget {
  /** Canonical source and exact revision, when supplied by the transport. */
  sourceId?: string;
  deviceDocumentId?: string;
  sourceKind?: 'local' | 'provider' | 'capture' | 'device';
  revision?: string;
  path: string;
  name: string;
  /** 1-based source line to land on (provenance jump); omitted opens at top. */
  line?: number;
  /**
   * 1-based last line of the matched chunk, so the host can mark what actually
   * matched rather than only where it starts. Omitted alongside `line`, and
   * omitted on targets that name a document rather than a chunk.
   */
  endLine?: number;
}

export interface SearchPaletteProps {
  /** Initial mode. `search` (Cmd+K) or `file` (Cmd+P). Defaults to `search`. */
  mode?: PaletteMode;
  /**
   * Which search intents to offer, in order. Defaults to all five. A host whose
   * backend cannot honestly serve an intent (e.g. a lexical-only static viewer
   * with no vector arm) passes a subset so it never presents fabricated results
   * under a Semantic/Recent label. The ⌘/Ctrl+N shortcuts and arrow-cycling both
   * follow this subset.
   */
  intents?: readonly SearchIntent[];
  /** The workspace's markdown files, for file mode's fuzzy jump. */
  files: PaletteFile[];
  /** The open document, for the Related intent (and breadcrumb context). */
  activeDoc: { path: string; name: string } | null;
  /** The workspace name — the breadcrumb's `source` segment. */
  sourceName: string | null;
  /**
   * The active workspace identity (its root). A change is a workspace switch:
   * the palette clears every result/error state immediately, so hits from the
   * previous workspace never linger under the new one — the belt to the
   * generation-tagged batch discard's suspenders.
   */
  workspaceKey?: string | null;
  /** Explicit filesystem root, used only for file-jump ranking and display. */
  workspaceRoot?: string | null;
  /** The search + related transport. */
  adapter: SearchAdapter;
  /** macOS chord glyphs (⌘) vs. Ctrl. */
  isMac: boolean;
  /** Open a search hit at its provenance line (Enter) or a background tab (⌘↵). */
  onOpenHit: (target: HitTarget, opts: { background: boolean }) => void;
  /** Open a file (file mode). */
  onOpenFile: (file: PaletteFile, opts: { background: boolean }) => void;
  /** Host navigation for an explicitly reported missing search scope. */
  onAddSource?: () => void;
  /** Dismiss the palette. */
  onClose: () => void;
  /** Test seam: force reduced motion regardless of the media query. */
  reducedMotion?: boolean;
  /** Test seam: debounce window in ms (default 120). */
  debounceMs?: number;
}

/** Read the reduced-motion preference (SSR/test-safe). */
function prefersReducedMotion(): boolean {
  return (
    typeof matchMedia !== 'undefined' && matchMedia('(prefers-reduced-motion: reduce)').matches
  );
}

/** The tab-focusable elements inside `root`, in document order — the modal's
 * focus ring (input, intent tabs, provenance breadcrumb buttons). */
function focusables(root: HTMLElement | null): HTMLElement[] {
  if (!root) return [];
  return Array.from(
    root.querySelectorAll<HTMLElement>(
      'button:not([disabled]):not([tabindex="-1"]), [href], input:not([disabled]), summary, [tabindex]:not([tabindex="-1"])',
    ),
  ).sort((left, right) => left.compareDocumentPosition(right) & Node.DOCUMENT_POSITION_FOLLOWING ? -1 : 1);
}

function errorMessage(cause: unknown): string {
  if (cause instanceof Error) return cause.message;
  return typeof cause === 'string' ? cause : 'Search failed';
}

/** Only the exact legacy no-term message is an empty result; other errors stay visible. */
function isNoSearchableTerms(message: string): boolean {
  return /^configuration error: query [\s\S]* contains no searchable terms$/.test(message);
}

/** A compact one-line explanation for the score badge's tooltip. */
function explainTitle(hit: SearchHit): string {
  const parts = [`score ${hit.score.toFixed(2)}`, `rrf ${hit.rrfScore.toFixed(2)}`];
  const { title, path, recency, graph } = hit.boosts;
  if (title > 0) parts.push(`title +${title.toFixed(2)}`);
  if (path > 0) parts.push(`path +${path.toFixed(2)}`);
  if (recency > 0) parts.push(`recency +${recency.toFixed(2)}`);
  if (graph > 0) parts.push(`graph +${graph.toFixed(2)}`);
  if (hit.lexRank != null) parts.push(`lex #${hit.lexRank}`);
  if (hit.semRank != null) parts.push(`sem #${hit.semRank}`);
  return parts.join(' · ');
}

/** The provenance breadcrumb: source › file › heading-path, segments clickable. */
function ProvenanceBreadcrumb({
  hit,
  sourceName,
  onJump,
}: {
  hit: SearchHit;
  sourceName: string | null;
  onJump: (crumb: Crumb) => void;
}) {
  const crumbs = buildBreadcrumb(hit, sourceName);
  return (
    <nav className="mdsearch-crumbs" aria-label="Provenance">
      {crumbs.map((crumb, index) => (
        <span className="mdsearch-crumbs__seg" key={`${crumb.kind}-${index}`}>
          {index > 0 ? <span className="mdsearch-crumbs__sep" aria-hidden="true">›</span> : null}
          {crumb.kind === 'source' ? (
            <span className="mdsearch-crumbs__source">{crumb.label}</span>
          ) : (
            <button
              type="button"
              className={`mdsearch-crumbs__link mdsearch-crumbs__link--${crumb.kind}`}
              // Keep the crumb click off the row's onSelect (which opens at the
              // section line); a crumb navigates on its own terms.
              onMouseDown={(event) => event.preventDefault()}
              onClick={(event) => {
                event.stopPropagation();
                onJump(crumb);
              }}
            >
              {crumb.label}
            </button>
          )}
        </span>
      ))}
    </nav>
  );
}

/** One search result: title, highlighted snippet, score badge, breadcrumb. */
function ResultRow({
  hit,
  sourceName,
  onJump,
}: {
  hit: SearchHit;
  sourceName: string | null;
  onJump: (crumb: Crumb) => void;
}) {
  const heading = hit.headingPath.length > 0 ? hit.headingPath[hit.headingPath.length - 1]! : null;
  const segments = parseSnippet(hit.snippet);
  return (
    <div className="mdsearch-row">
      <div className="mdsearch-row__head">
        <span className="mdsearch-row__title">
          {hit.name}
          {heading ? <span className="mdsearch-row__heading"> · {heading}</span> : null}
        </span>
        <span className="mdsearch-row__badge" title={explainTitle(hit)}>
          {hit.score.toFixed(2)}
        </span>
      </div>
      <p className="mdsearch-row__snippet">
        {segments.map((seg, index) =>
          seg.match ? (
            <mark className="mdsearch-row__mark" key={index}>
              {seg.text}
            </mark>
          ) : (
            <span key={index}>{seg.text}</span>
          ),
        )}
      </p>
      <ProvenanceBreadcrumb hit={hit} sourceName={sourceName} onJump={onJump} />
    </div>
  );
}

/** One related-document row with understandable relationship reasons. */
function RelatedRow({ hit }: { hit: RelatedHit }) {
  const reasons = relatedReasons(hit);
  return (
    <div className="mdsearch-row">
      <div className="mdsearch-row__head">
        <span className="mdsearch-row__title">{hit.name}</span>
      </div>
      <div className="mdsearch-row__components">
        {reasons.map(reason => <span className="mdsearch-chip" key={reason}>{reason}</span>)}
      </div>
      <div className="mdsearch-crumbs">
        <span className="mdsearch-crumbs__source">{hit.filePath || hit.name}</span>
      </div>
    </div>
  );
}

export function SearchPalette({
  mode = 'search',
  intents,
  files,
  activeDoc,
  sourceName,
  workspaceKey,
  workspaceRoot,
  adapter,
  isMac,
  onOpenHit,
  onOpenFile,
  onClose,
  onAddSource,
  reducedMotion,
  debounceMs = 120,
}: SearchPaletteProps) {
  // The intents this host offers, in order. Defaults to all five; a subset drops
  // the intents the host's backend can't honestly serve.
  const enabledIntents = useMemo(() => {
    if (!intents) return INTENTS;
    const set = new Set(intents);
    const filtered = INTENTS.filter((meta) => set.has(meta.id));
    return filtered.length > 0 ? filtered : INTENTS;
  }, [intents]);

  const [query, setQuery] = useState('');
  const [intent, setIntent] = useState<SearchIntent>(
    () => enabledIntents[0]?.id ?? 'best',
  );
  const [selectedValue, setSelectedValue] = useState('');

  const [lexHits, setLexHits] = useState<SearchHit[]>([]);
  const [finalHits, setFinalHits] = useState<SearchHit[] | null>(null);
  const [relatedHits, setRelatedHits] = useState<RelatedHit[]>([]);
  const [relatedNotice, setRelatedNotice] = useState<string | null>(null);
  const [filterError, setFilterError] = useState<string | null>(null);
  // Why the shown ranking is not the one that was asked for (the semantic arm
  // could not run). Never inferred from the hits — only ever what the adapter
  // reported, so an empty or short result set can't be silently rationalised.
  const [degraded, setDegraded] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [merging, setMerging] = useState(false);

  const runIdRef = useRef(0);
  const mergeTimer = useRef<number | undefined>(undefined);
  const overlayRef = useRef<HTMLDivElement | null>(null);
  const noMotion = reducedMotion ?? prefersReducedMotion();

  const isFileMode = mode === 'file';
  const plan = intentPlan(intent);
  const isRelated = !isFileMode && plan.mode === 'related';
  const { coverage, publish: publishCoverage } = useSearchCoverage(query, intent, mode, adapter, workspaceKey);
  const scopeRequest = useMemo(() => ({}), [query, intent, mode, adapter, workspaceKey]);
  const [missingScope, setMissingScope] = useState<object | null>(null);
  const scopeMissing = !isFileMode && !isRelated && missingScope === scopeRequest;

  // Belt: a workspace switch clears every result/error state at once, so hits
  // from the previous workspace never linger in the new one's palette. The run
  // effect keys on workspaceKey too, so its in-flight callbacks are superseded.
  useEffect(() => {
    setLexHits([]);
    setFinalHits(null);
    setRelatedHits([]);
    setFilterError(null);
    setDegraded(null);
    setRelatedNotice(null);
    setSelectedValue('');
  }, [workspaceKey]);

  // Nudge the (subtle) reorder cue when the semantic pass supersedes lexical.
  const triggerMerge = useCallback(() => {
    if (noMotion) return;
    setMerging(true);
    window.clearTimeout(mergeTimer.current);
    mergeTimer.current = window.setTimeout(() => setMerging(false), 320);
  }, [noMotion]);
  useEffect(() => () => window.clearTimeout(mergeTimer.current), []);

  // Run a search / related lookup whenever the query, intent, or context moves.
  useEffect(() => {
    if (isFileMode) return; // file mode is synchronous (see `fileMatches`)
    const id = (runIdRef.current += 1);
    setFilterError(null);
    setMissingScope(null);
    setDegraded(null);
    setRelatedNotice(null);
    // A previous Best/Exact list must not survive under the Semantic tab while
    // the vector request is pending. Semantic has no lexical preview phase.
    if (intent === 'semantic') {
      setLexHits([]);
      setFinalHits(null);
    }

    if (plan.mode === 'related') {
      if (!activeDoc) {
        setRelatedHits([]);
        return;
      }
      setBusy(true);
      adapter
        .related(activeDoc.path)
        .then((hits) => {
          if (runIdRef.current === id) {
            setRelatedHits(unwrapRelated(hits));
            setRelatedNotice(relatedAvailabilityMessage(hits));
          }
        })
        .catch((cause) => {
          if (runIdRef.current === id) setFilterError(errorMessage(cause));
        })
        .finally(() => {
          if (runIdRef.current === id) setBusy(false);
        });
      return;
    }

    // Empty query: there is nothing to search. Clear results/busy and let the
    // "Type to search" empty state show, rather than firing `search("")` — which
    // the engine (correctly) rejects as "no searchable terms", surfacing a red
    // error banner over the empty state. Filter-only queries (`tag:`, `path:`)
    // carry non-empty trimmed text, so they fall through and STILL search (the
    // engine's listing path handles them). filterError was already cleared above.
    if (query.trim().length === 0) {
      setLexHits([]);
      setFinalHits(null);
      setBusy(false);
      return;
    }

    const handle = window.setTimeout(() => {
      setBusy(true);
      setFinalHits(null);
      adapter
        .search(query, { lexicalOnly: plan.lexicalOnly, explain: true, intent }, (phase, hits) => {
          if (runIdRef.current !== id) return;
          if (phase === 'lexical') {
            if (intent === 'semantic') return;
            setLexHits(sortHits(intent, hits));
          } else {
            const ranked = intent === 'semantic' ? hits.filter((hit) => hit.semRank !== null) : hits;
            setFinalHits(sortHits(intent, ranked));
            triggerMerge();
          }
        })
        .then((outcome) => {
          if (runIdRef.current !== id) return;
          let hits = outcome.hits;
          if (intent === 'semantic') {
            hits = outcome.semanticUnavailable ? [] : hits.filter((hit) => hit.semRank !== null);
          }
          setFinalHits(sortHits(intent, hits));
          // The resolved outcome is authoritative (the streamed phases are
          // best-effort), so the honesty of the shown list is decided here.
          setDegraded(outcome.semanticUnavailable);
          publishCoverage(outcome.coverage);
        })
        .catch((cause) => {
          if (runIdRef.current !== id) return;
          const message = errorMessage(cause);
          // A non-empty query the engine finds has no searchable terms (e.g.
          // only punctuation) is not a filter error worth spelling out — show
          // the "No results" empty state, not the raw configuration-error banner.
          const noScope = cause instanceof SearchScopeMissingError;
          setMissingScope(noScope ? scopeRequest : null);
          setFilterError(noScope || isNoSearchableTerms(message) ? null : message);
          setLexHits([]);
          setFinalHits([]);
          setDegraded(null);
          setRelatedNotice(null);
          publishCoverage(null);
        })
        .finally(() => {
          if (runIdRef.current === id) setBusy(false);
        });
    }, debounceMs);
    return () => window.clearTimeout(handle);
  }, [query, intent, mode, activeDoc, adapter, debounceMs, isFileMode, plan.lexicalOnly, plan.mode, triggerMerge, workspaceKey, publishCoverage, scopeRequest]);

  const fileMatches = useMemo(
    () => (isFileMode ? rankFiles(files, query, 100, workspaceRoot) : []),
    [isFileMode, files, query, workspaceRoot],
  );

  const searchHits = finalHits ?? lexHits;
  const relatedShown = useMemo(() => filterRelated(relatedHits, query), [relatedHits, query]);

  // value → open target, so ⌘↵ (background) can resolve the highlighted item.
  const targets = useMemo(() => {
    const map = new Map<string, HitTarget | { file: PaletteFile }>();
    if (isFileMode) {
      for (const { file } of fileMatches) map.set(file.path, { file });
    } else if (isRelated) {
      for (const hit of relatedShown) {
        map.set(hit.path.toLowerCase(), { path: hit.path, name: hit.name, ...(hit.sourceId != null ? { sourceId: hit.sourceId } : {}),
          ...(hit.deviceDocumentId != null ? { deviceDocumentId: hit.deviceDocumentId } : {}),
          ...(hit.sourceKind != null ? { sourceKind: hit.sourceKind } : {}), ...(hit.revision != null ? { revision: hit.revision } : {}), ...(hit.line != null ? { line: hit.line } : {}) });
      }
    } else {
      for (const hit of searchHits) {
        map.set(hit.chunkId.toLowerCase(), {
          path: hit.path,
          name: hit.name,
          ...(hit.sourceId != null ? { sourceId: hit.sourceId } : {}),
          ...(hit.deviceDocumentId != null ? { deviceDocumentId: hit.deviceDocumentId } : {}),
          ...(hit.sourceKind != null ? { sourceKind: hit.sourceKind } : {}),
          ...(hit.revision != null ? { revision: hit.revision } : {}),
          line: hit.startLine,
          endLine: hit.endLine,
        });
      }
    }
    return map;
  }, [isFileMode, isRelated, fileMatches, relatedShown, searchHits]);

  const openValue = useCallback(
    (value: string, background: boolean) => {
      const target = targets.get(isFileMode ? value : value.toLowerCase());
      if (!target) return;
      if ('file' in target) onOpenFile(target.file, { background });
      else onOpenHit(target, { background });
      if (!background) onClose();
    },
    [targets, isFileMode, onOpenFile, onOpenHit, onClose],
  );

  const onCrumbJump = useCallback(
    (hit: SearchHit, crumb: Crumb) => {
      // The `file` crumb names the DOCUMENT, so it opens at the top with no line
      // at all — deliberately, not by losing one. Every other crumb names the
      // chunk, and carries the chunk's whole range.
      const chunk = crumb.kind !== 'file';
      onOpenHit(
        {
          path: hit.path,
          name: hit.name,
          ...(hit.sourceId != null ? { sourceId: hit.sourceId } : {}),
          ...(hit.deviceDocumentId != null ? { deviceDocumentId: hit.deviceDocumentId } : {}),
          ...(hit.sourceKind != null ? { sourceKind: hit.sourceKind } : {}),
          ...(hit.revision != null ? { revision: hit.revision } : {}),
          line: chunk ? hit.startLine : undefined,
          endLine: chunk ? hit.endLine : undefined,
        },
        { background: false },
      );
      onClose();
    },
    [onOpenHit, onClose],
  );

  // Move the intent selection to a specific button (roving focus within the
  // tablist), after the re-render that marks it active.
  const focusIntent = useCallback((id: SearchIntent) => {
    requestAnimationFrame(() => {
      overlayRef.current?.querySelector<HTMLElement>(`[data-intent="${id}"]`)?.focus();
    });
  }, []);

  // Container-level keys (capture phase, so ⌘↵ pre-empts cmdk's plain Enter).
  // Bare Tab is NOT captured here — it is a real focus move (trapped within the
  // overlay), which is what keeps the intent tabs and provenance breadcrumb
  // buttons reachable by keyboard. Intents switch on ⌘/Ctrl+1..5 instead.
  const onKeyDownCapture = useCallback(
    (event: React.KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault();
        onClose();
        return;
      }
      // ⌘/Ctrl+Enter → open the highlighted item in a background tab.
      if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) {
        event.preventDefault();
        event.stopPropagation();
        if (selectedValue) openValue(selectedValue, true);
        return;
      }
      // ⌘/Ctrl+1..N → jump straight to an enabled intent (search mode only).
      if (!isFileMode && (event.metaKey || event.ctrlKey) && /^[1-9]$/.test(event.key)) {
        const index = Number(event.key) - 1;
        if (index < enabledIntents.length) {
          event.preventDefault();
          const next = enabledIntents[index]!.id;
          setIntent(next);
          focusIntent(next);
          return;
        }
      }
      // Focus trap: keep Tab within the overlay (wrap at the ends) so focus can
      // reach every control and never escapes to the page behind the modal.
      if (event.key === 'Tab') {
        const nodes = focusables(overlayRef.current);
        if (nodes.length === 0) return;
        const first = nodes[0]!;
        const last = nodes[nodes.length - 1]!;
        const active = document.activeElement;
        if (event.shiftKey && active === first) {
          event.preventDefault();
          last.focus();
        } else if (!event.shiftKey && active === last) {
          event.preventDefault();
          first.focus();
        }
      }
    },
    [isFileMode, onClose, openValue, selectedValue, focusIntent, enabledIntents],
  );

  // Arrow keys rove within the (enabled) intent tablist (the ARIA pattern).
  const onIntentKeyDown = useCallback(
    (event: React.KeyboardEvent, id: SearchIntent) => {
      if (event.key !== 'ArrowRight' && event.key !== 'ArrowLeft') return;
      event.preventDefault();
      const dir = event.key === 'ArrowRight' ? 1 : -1;
      const current = enabledIntents.findIndex((meta) => meta.id === id);
      const base = current < 0 ? 0 : current;
      const next =
        enabledIntents[(base + dir + enabledIntents.length) % enabledIntents.length]!.id;
      setIntent(next);
      focusIntent(next);
    },
    [focusIntent, enabledIntents],
  );

  const modKey = isMac ? '⌘' : 'Ctrl';
  const placeholder = isFileMode
    ? 'Jump to a file…'
    : isRelated
      ? 'Filter related notes…'
      : 'Search document content…';

  return (
    <div
      className="mdsearch-backdrop"
      role="presentation"
      onMouseDown={onClose}
      onKeyDownCapture={onKeyDownCapture}
    >
      <div
        ref={overlayRef}
        className={`mdsearch${merging ? ' mdsearch--merging' : ''}`}
        role="dialog"
        aria-modal="true"
        aria-label={isFileMode ? 'Jump to file' : 'Search content'}
        onMouseDown={(event) => event.stopPropagation()}
      >
        <Command
          shouldFilter={false}
          loop
          label={isFileMode ? 'Jump to file' : 'Search content'}
          value={selectedValue}
          onValueChange={setSelectedValue}
        >
          <div className="mdsearch__context">
            <strong>{isFileMode ? 'Jump to file' : 'Search content'}</strong>
            <span>{isFileMode ? 'Filenames and folders' : 'Indexed document text'}{sourceName ? ` · ${sourceName}` : ''}</span>
          </div>
          {isFileMode ? null : (
            <div className="mdsearch__intents" role="tablist" aria-label="Search intent">
              {enabledIntents.map((meta, index) => (
                <button
                  key={meta.id}
                  type="button"
                  role="tab"
                  data-intent={meta.id}
                  aria-selected={meta.id === intent}
                  aria-keyshortcuts={`${isMac ? 'Meta' : 'Control'}+${index + 1}`}
                  tabIndex={meta.id === intent ? 0 : -1}
                  className={`mdsearch__intent${meta.id === intent ? ' mdsearch__intent--active' : ''}`}
                  title={`${meta.hint} (${modKey}${index + 1})`}
                  onMouseDown={(event) => event.preventDefault()}
                  onClick={() => setIntent(meta.id)}
                  onKeyDown={(event) => onIntentKeyDown(event, meta.id)}
                >
                  {meta.label}
                </button>
              ))}
              <span className="mdsearch__intents-hint" aria-hidden="true">
                {modKey}1–{enabledIntents.length} · ←→
              </span>
            </div>
          )}

          <Command.Input
            className="mdsearch__input"
            value={query}
            onValueChange={setQuery}
            placeholder={placeholder}
            aria-label={isFileMode ? 'Jump to file' : 'Search content'}
            autoFocus
          />

          {scopeMissing ? <div className="mdsearch__no-source" role="status">
            <p>Add a source to search its Markdown content.</p>
            {onAddSource ? <button type="button" onClick={onAddSource}>Add source</button> : null}
          </div> : null}

          {filterError ? (
            <div className="mdsearch__error" role="alert">
              {filterError}
            </div>
          ) : null}

          {degraded ? (
            // Best can display explicitly qualified lexical fallback; Semantic
            // reports unavailability without displaying a different ranking.
            <div className="mdsearch__degraded" role="status">
              {intent === 'semantic'
                ? `Semantic search is unavailable (${degraded})`
                : `Text matches only — semantic ranking is unavailable (${degraded})`}
            </div>
          ) : null}

          {!isFileMode && !isRelated && coverage ? <SearchCoveragePanel coverage={coverage} /> : null}

          <Command.List className="mdsearch__list">
            {isFileMode ? (
              <>
                <Command.Empty className="mdsearch__empty">No matching files</Command.Empty>
                {fileMatches.map(({ file, context }) => (
                  <Command.Item
                    key={file.path}
                    value={file.path}
                    className="mdsearch__item"
                    title={file.path}
                    aria-label={`${file.name}, ${context.relativePath ?? context.parentLabel}. Full path: ${file.path}`}
                    onSelect={(value) => openValue(value, false)}
                  >
                    <span className="mdsearch-file__name">{file.name}</span>
                    <span className="mdsearch-file__path" title={context.relativePath ?? file.path}>{context.parentLabel}</span>
                  </Command.Item>
                ))}
              </>
            ) : isRelated ? (
              <>
                {!activeDoc ? (
                  <div className="mdsearch__empty">Open a document to see related notes</div>
                ) : (
                  <Command.Empty className="mdsearch__empty">
                    {busy ? 'Finding related notes…' : 'No related notes'}
                  </Command.Empty>
                )}
                {relatedNotice ? <div role="status" className="mdsearch__empty">{relatedNotice}</div> : null}
                {relatedShown.map((hit) => (
                  <Command.Item
                    key={hit.path}
                    value={hit.path}
                    className="mdsearch__item"
                    onSelect={(value) => openValue(value, false)}
                  >
                    <RelatedRow hit={hit} />
                  </Command.Item>
                ))}
              </>
            ) : (
              <>
                <Command.Empty className="mdsearch__empty">
                  {scopeMissing ? null : busy ? 'Searching…' : query.trim().length === 0 ? 'Type to search' : 'No results'}
                </Command.Empty>
                {searchHits.map((hit) => (
                  <Command.Item
                    key={hit.chunkId}
                    value={hit.chunkId}
                    className="mdsearch__item"
                    onSelect={(value) => openValue(value, false)}
                  >
                    <ResultRow
                      hit={hit}
                      sourceName={sourceName}
                      onJump={(crumb) => onCrumbJump(hit, crumb)}
                    />
                  </Command.Item>
                ))}
              </>
            )}
          </Command.List>

          <div className="mdsearch__footer" aria-hidden="true">
            <span>
              <kbd>↵</kbd> open
            </span>
            <span>
              <kbd>{modKey}↵</kbd> background tab
            </span>
            {isFileMode ? null : (
              <span>
                <kbd>
                  {modKey}1–{enabledIntents.length}
                </kbd>{' '}
                intent
              </span>
            )}
            <span>
              <kbd>Esc</kbd> close
            </span>
          </div>
        </Command>
      </div>
    </div>
  );
}
