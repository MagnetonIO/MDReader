/**
 * The SearchPalette's pure model: the intent vocabulary and every framework-free
 * transformation the component leans on (intent → engine plan, recency sort,
 * FTS-snippet parsing, provenance-breadcrumb assembly, intent cycling). Kept
 * free of React so each piece is unit-tested directly and the website (P5) can
 * reuse the same logic behind its own transport.
 */

// --------------------------------------------------------------------------
// Wire-normalized shapes (the transport adapter produces these)
// --------------------------------------------------------------------------

/** The five search intents, in segmented-control order. */
export type SearchIntent = 'best' | 'exact' | 'semantic' | 'recent' | 'related';

/** Recorded boost contributions behind a hit's score. */
export interface HitBoosts {
  title: number;
  path: number;
  recency: number;
  graph: number;
}

/** The full per-hit score breakdown (present when explain was requested). */
export interface SearchExplain {
  lexRank: number | null;
  semRank: number | null;
  rrfScore: number;
  boosts: HitBoosts;
  finalScore: number;
  indexVersion: string | null;
  embeddingModel: string | null;
}

/** One search hit, transport-normalized for the palette. */
export interface SearchHit {
  chunkId: string;
  documentId: string;
  /** Absolute path the reader opens. */
  path: string;
  /** Source-relative stored path (the breadcrumb's file segment). */
  filePath: string;
  /** File name (basename). */
  name: string;
  /** Heading breadcrumb of the chunk's section. */
  headingPath: string[];
  /** First source line of the section — the provenance jump target. */
  startLine: number;
  /** Last source line of the section. */
  endLine: number;
  /** Final score (RRF + boosts). */
  score: number;
  /** Fused RRF score before boosts. */
  rrfScore: number;
  /** Recorded boost contributions. */
  boosts: HitBoosts;
  /** 1-based lexical rank, if the chunk appeared in the FTS results. */
  lexRank: number | null;
  /** 1-based semantic rank, if the chunk appeared in the vector results. */
  semRank: number | null;
  /** Preview snippet, FTS-highlighted with `[...]` markers when lexical. */
  snippet: string;
  /** Document mtime, ms since the Unix epoch (the Recent intent sorts on it). */
  mtime: number;
  /** Full breakdown, present when explain was requested. */
  explain: SearchExplain | null;
}

/** Per-component contributions behind a related score. */
export interface RelatedComponents {
  graph: number;
  concept: number;
  vector: number;
}

/** One related document, transport-normalized. */
export interface RelatedHit {
  documentId: string;
  /** Absolute path the reader opens. */
  path: string;
  /** Source-relative stored path. */
  filePath: string;
  name: string;
  score: number;
  components: RelatedComponents;
}

/**
 * What a search produced, and whether it is the ranking that was asked for.
 *
 * The hits alone cannot say this: a hybrid pass that lost its vector arm returns
 * fewer, differently-ordered hits that look exactly like an ordinary result set.
 * So the outcome carries the distinction explicitly — and the hits are reached
 * *through* it, so a caller cannot render a degraded list without having been
 * handed the reason it is degraded.
 */
export interface SearchOutcome {
  /** The final hits, best-first. */
  hits: SearchHit[];
  /**
   * `null` when the ranking is the one requested — a full hybrid pass, or a
   * lexical-only pass the caller asked for (the Exact intent). Otherwise the
   * reason the semantic arm could not run (typically: the embedding model could
   * not be loaded), meaning `hits` is lexical-only and must be shown as such.
   */
  semanticUnavailable: string | null;
}

/**
 * The transport the palette drives — Tauri IPC in the desktop app, HTTP on the
 * website (P5). `search` streams two honest phases through `onPhase` (lexical
 * first, then the hybrid final) and resolves to the final {@link SearchOutcome};
 * a rejected promise carries the schema-derived filter-error text.
 */
export interface SearchAdapter {
  search: (
    query: string,
    opts: { lexicalOnly: boolean; explain: boolean },
    onPhase: (phase: 'lexical' | 'final', hits: SearchHit[]) => void,
  ) => Promise<SearchOutcome>;
  related: (docPath: string) => Promise<RelatedHit[]>;
}

// --------------------------------------------------------------------------
// Intents
// --------------------------------------------------------------------------

/** Display metadata for one intent (the segmented control reads this). */
export interface IntentMeta {
  id: SearchIntent;
  label: string;
  hint: string;
}

/** The intents in their segmented-control order. */
export const INTENTS: readonly IntentMeta[] = [
  { id: 'best', label: 'Best', hint: 'Hybrid: lexical + semantic, fused' },
  { id: 'exact', label: 'Exact', hint: 'Lexical only — literal term matches' },
  { id: 'semantic', label: 'Semantic', hint: 'Meaning-weighted, vectors lead' },
  { id: 'recent', label: 'Recent', hint: 'Newest-modified first' },
  { id: 'related', label: 'Related', hint: 'Notes related to the open document' },
];

/** How an intent maps onto the engine call. */
export interface IntentPlan {
  /** `related` calls related(); everything else searches. */
  mode: 'search' | 'related';
  /** Pass `lexical_only` to the engine (the Exact intent, single phase). */
  lexicalOnly: boolean;
  /** Re-sort the hits by document mtime, newest first (the Recent intent). */
  recencySort: boolean;
}

/** The engine plan for an intent. */
export function intentPlan(intent: SearchIntent): IntentPlan {
  switch (intent) {
    case 'exact':
      return { mode: 'search', lexicalOnly: true, recencySort: false };
    case 'recent':
      return { mode: 'search', lexicalOnly: false, recencySort: true };
    case 'related':
      return { mode: 'related', lexicalOnly: false, recencySort: false };
    case 'semantic':
    case 'best':
    default:
      return { mode: 'search', lexicalOnly: false, recencySort: false };
  }
}

/** The next intent when Tab (`dir=1`) / Shift+Tab (`dir=-1`) cycles. Wraps. */
export function cycleIntent(current: SearchIntent, dir: 1 | -1): SearchIntent {
  const index = INTENTS.findIndex((meta) => meta.id === current);
  const base = index < 0 ? 0 : index;
  const next = (base + dir + INTENTS.length) % INTENTS.length;
  return INTENTS[next]!.id;
}

// --------------------------------------------------------------------------
// Result ordering
// --------------------------------------------------------------------------

/**
 * Order hits for `intent`: the Recent intent sorts by document mtime (newest
 * first, ties broken by score then path for stability); every other intent
 * keeps the engine's relevance order. Always returns a NEW array (never mutates
 * the input — the streamed phases hand the same reference around).
 */
export function sortHits(intent: SearchIntent, hits: SearchHit[]): SearchHit[] {
  if (!intentPlan(intent).recencySort) return hits.slice();
  return hits.slice().sort(
    (a, b) => b.mtime - a.mtime || b.score - a.score || a.path.localeCompare(b.path),
  );
}

/** Filter a related list by a query substring over name/path (empty → all). */
export function filterRelated(hits: RelatedHit[], query: string): RelatedHit[] {
  const needle = query.trim().toLowerCase();
  if (needle.length === 0) return hits.slice();
  return hits.filter(
    (hit) =>
      hit.name.toLowerCase().includes(needle) || hit.filePath.toLowerCase().includes(needle),
  );
}

// --------------------------------------------------------------------------
// Snippet highlighting
// --------------------------------------------------------------------------

/** One run of snippet text, `match` when it sat inside FTS `[...]` markers. */
export interface SnippetSegment {
  text: string;
  match: boolean;
}

/**
 * Split an FTS snippet into plain/highlighted runs. sqlite's `snippet()` wraps
 * matched terms in `[` … `]` (never HTML), so this parses those markers into
 * safe text segments the row renders as `<mark>` — no `dangerouslySetInnerHTML`,
 * no injection surface. Unbalanced or literal brackets degrade to plain text.
 */
export function parseSnippet(snippet: string): SnippetSegment[] {
  const segments: SnippetSegment[] = [];
  let buffer = '';
  let inMatch = false;
  const flush = (match: boolean) => {
    if (buffer.length > 0) segments.push({ text: buffer, match });
    buffer = '';
  };
  for (const ch of snippet) {
    if (!inMatch && ch === '[') {
      flush(false);
      inMatch = true;
    } else if (inMatch && ch === ']') {
      flush(true);
      inMatch = false;
    } else {
      buffer += ch;
    }
  }
  // Trailing buffer: an unbalanced `[` never closed → treat as plain text.
  flush(inMatch ? false : false);
  return segments.length > 0 ? segments : [{ text: snippet, match: false }];
}

// --------------------------------------------------------------------------
// Provenance breadcrumb
// --------------------------------------------------------------------------

/** One clickable (or, for the source, static) breadcrumb segment. */
export interface Crumb {
  kind: 'source' | 'file' | 'heading';
  label: string;
  /** For heading crumbs, its 0-based index in the heading path. */
  headingIndex?: number;
}

/**
 * Assemble the provenance breadcrumb for a hit: `source › file › heading-path`.
 * The source is the workspace name (static context); the file and each heading
 * are clickable segments that jump into the document. A hit with no source name
 * omits the source crumb; a headingless chunk omits heading crumbs.
 */
export function buildBreadcrumb(
  hit: Pick<SearchHit, 'filePath' | 'name' | 'headingPath'>,
  sourceName: string | null,
): Crumb[] {
  const crumbs: Crumb[] = [];
  if (sourceName && sourceName.length > 0) {
    crumbs.push({ kind: 'source', label: sourceName });
  }
  crumbs.push({ kind: 'file', label: hit.filePath || hit.name });
  hit.headingPath.forEach((heading, index) => {
    crumbs.push({ kind: 'heading', label: heading, headingIndex: index });
  });
  return crumbs;
}
