import { normalizeSearchCoverage } from './search-coverage';
/**
 * Thin, ergonomic wrappers over the reader backend's workspace and search
 * commands (`./backend`). The `Result`-returning commands hand back a
 * `{ status: 'ok' | 'error' }` union; these helpers unwrap that union, throwing
 * on error, so the stores and components consume plain promises.
 */

import { SearchScopeMissingError } from '@mdreader/ui';
import type {
  RelatedHit, RelatedOutcome, SearchAdapter, SearchHit, SearchIntent, SearchOutcome,
} from '@mdreader/ui';

import { readerBackend, unwrapNative } from './backend';
import type {
  FileEntry,
  DocPayload,
  WorkspaceOriginReceipt,
  SaveOutcome as SaveOutcomeDto,
  FsChanged,
  WatchError,
  ImportProgress,
  ListingError,
  WikiResolution as BindingWikiResolution,
  SearchHitDto,
  SearchBatch,
  RelatedDocDto,
  RelatedOutcomeDto,
} from '../bindings';

export type { FileEntry, DocPayload, FsChanged, WatchError };
export type { SearchHit, RelatedHit, SearchOutcome } from '@mdreader/ui';

/** A `blake3`/`f64`-normalised document payload (never-null numbers). */
export interface Doc {
  content: string;
  mtime: number;
  hash: string;
  /** Native saved document in one retained workspace grant era. */
  originReceipt?: WorkspaceOriginReceipt;
}

/** A workspace file with numbers coerced from the `number | null` binding. */
export interface WorkspaceFile {
  path: string;
  name: string;
  dir: string;
  mtime: number;
  size: number;
  /** True when a gitignore rule matched the file — the tree dims it. */
  ignored: boolean;
}

/** A listing plus which side produced it (engine index vs live fs fallback). */
export interface WorkspaceListing {
  files: WorkspaceFile[];
  fromIndex: boolean;
}

/**
 * Why a listing failed — the UI-side mirror of the backend's `ListingError`.
 *
 * `unreadable` is the OS refusing to read the folder (on macOS, the privacy
 * layer withholding a protected folder from an app with no grant). It is kept
 * separate from every other failure because it is the only one the *user* can
 * fix, and because it must never be shown as an empty folder — the folder is
 * not empty, we just cannot see into it.
 */
export type ListingFailure =
  | { kind: 'unreadable'; path: string; reason: string; remedy: string }
  | { kind: 'failed'; message: string };

/** A listing failure carried on a thrown `Error`, so callers can name the cause. */
export class ListingFailedError extends Error {
  readonly failure: ListingFailure;

  constructor(failure: ListingFailure) {
    super(
      failure.kind === 'unreadable'
        ? `${failure.path} could not be read: ${failure.reason}`
        : failure.message,
    );
    this.name = 'ListingFailedError';
    this.failure = failure;
  }
}

/** Narrow an unknown thrown value to its listing failure, if it carries one. */
export function listingFailureOf(cause: unknown): ListingFailure | null {
  return cause instanceof ListingFailedError ? cause.failure : null;
}

/** Normalise the backend's tagged error into the UI-side union. A payload that
 *  is not the tagged shape (a mocked IPC, an older backend) degrades to
 *  `failed` rather than being mistaken for a readable-but-empty folder. */
function normalizeListingError(error: ListingError | string): ListingFailure {
  if (typeof error === 'string') return { kind: 'failed', message: error };
  if (error.kind === 'unreadable') {
    return {
      kind: 'unreadable',
      path: error.path,
      reason: error.reason,
      remedy: error.remedy,
    };
  }
  return { kind: 'failed', message: error.message };
}

/** The canonical grant plus the generation its watcher was armed under. */
export interface WorkspaceGrant {
  root: string;
  generation: number;
}

/** Import phase for the StatusBar indicator. `loading` covers the one-time
 *  embedding-model construction/download that precedes the first import;
 *  `reembedding` is an import that is re-embedding the whole workspace because
 *  the embedding model changed (a one-time cost the generic `importing` label
 *  would understate); `model-unavailable` is that model failing to load at all —
 *  its own phase because the workspace remains readable and text indexing is
 *  independent of optional semantic readiness. */
export type ImportPhase =
  | 'idle'
  | 'loading'
  | 'importing'
  | 'reembedding'
  | 'done'
  | 'model-unavailable'
  | 'error';

/** A normalised import-progress snapshot (camelCased, never-null counts). */
export interface ImportState {
  phase: ImportPhase;
  filesDone: number;
  filesTotal: number;
  chunks: number;
  error: string | null;
}

/** A wikilink resolution result (candidates carry never-null paths). */
export type WikiResolution =
  | { status: 'resolved'; path: string; name: string; line: number | null }
  | { status: 'ambiguous'; candidates: { path: string; name: string }[] }
  | { status: 'unresolved' };

function normalizeFile(entry: FileEntry): WorkspaceFile {
  return {
    path: entry.path,
    name: entry.name,
    dir: entry.dir,
    mtime: entry.mtime ?? 0,
    size: entry.size ?? 0,
    ignored: entry.ignored,
  };
}

function normalizeProgress(progress: ImportProgress): ImportState {
  const phase = progress.phase;
  return {
    phase:
      phase === 'loading' ||
      phase === 'importing' ||
      phase === 'reembedding' ||
      phase === 'done' ||
      phase === 'model-unavailable' ||
      phase === 'error'
        ? phase
        : 'idle',
    filesDone: progress.files_done,
    filesTotal: progress.files_total,
    chunks: progress.chunks_embedded,
    error: progress.error,
  };
}

function normalizeResolution(resolution: BindingWikiResolution): WikiResolution {
  if (resolution.status === 'resolved' && resolution.path && resolution.name) {
    return {
      status: 'resolved',
      path: resolution.path,
      name: resolution.name,
      line: resolution.line,
    };
  }
  if (resolution.status === 'ambiguous') {
    return { status: 'ambiguous', candidates: resolution.candidates };
  }
  return { status: 'unresolved' };
}

/**
 * List the markdown files under `root`, from the engine index once an import has
 * finished (gitignored files already dropped) or a live filesystem scan before
 * then (gitignored files flagged for dimming).
 *
 * Throws a [`ListingFailedError`] on backend error, carrying the typed cause —
 * in particular whether the folder could not be READ. Callers must never turn
 * that throw into an empty file list: an unreadable folder is not an empty one.
 */
export async function listWorkspaceFiles(root: string): Promise<WorkspaceListing> {
  const result = await readerBackend().workspace.listWorkspaceFiles(root);
  if (result.status === 'error') throw new ListingFailedError(normalizeListingError(result.error));
  return {
    files: result.data.entries.map(normalizeFile),
    fromIndex: result.data.from_index,
  };
}

/**
 * Open (or reuse) the workspace index and run an incremental import, streaming
 * coarse progress to `onProgress` as it goes and resolving to the final status.
 * The channel feed is best-effort; the resolved value is authoritative. Throws
 * on backend error.
 */
export async function startImport(
  root: string,
  onProgress: (state: ImportState) => void,
): Promise<ImportState> {
  const result = await readerBackend().workspace.startImport(root, (progress) => onProgress(normalizeProgress(progress)));
  if (result.status === 'error') throw new Error(result.error);
  return normalizeProgress(result.data);
}

/** The latest import status (for initial render / polling). Throws on error. */
export async function importStatus(): Promise<ImportState> {
  const result = await readerBackend().workspace.importStatus();
  if (result.status === 'error') throw new Error(result.error);
  return normalizeProgress(result.data);
}

/**
 * Resolve a `[[wikilink]]` against the index: exact filename then title match,
 * ambiguity returning the candidate set. `heading` (the `#heading` suffix)
 * resolves to a source line in the target. Throws on backend error.
 */
export async function resolveWikilink(
  fromDoc: string,
  target: string,
  heading?: string | null,
): Promise<WikiResolution> {
  const result = await readerBackend().workspace.resolveWikilink(fromDoc, target, heading ?? null);
  if (result.status === 'error') throw new Error(result.error);
  return normalizeResolution(result.data);
}

/** Read a single document. Throws on backend error. */
export async function readDoc(path: string): Promise<Doc> {
  const result = await readerBackend().workspace.readDoc(path);
  if (result.status === 'error') throw new Error(result.error);
  const payload: DocPayload = result.data;
  const originReceipt = payload.originReceipt;
  if (originReceipt && (originReceipt.hash !== payload.hash || !/^[a-f0-9]{64}$/.test(originReceipt.hash)
    || !/^[a-f0-9-]{36}$/.test(originReceipt.epoch) || !originReceipt.path || originReceipt.path.startsWith('/')
    || /[:\\\u0000-\u001f\u007f]/.test(originReceipt.path)
    || originReceipt.path.split('/').some((part) => !part || part === '.' || part === '..'))) {
    throw new Error('The document folder receipt could not be verified.');
  }
  return { content: payload.content, mtime: payload.mtime ?? 0, hash: payload.hash, originReceipt };
}

/**
 * A hash-guarded save outcome — a proper discriminated union. A `conflict` is an
 * expected result (the file changed on disk since it was opened), carrying the
 * hash the file actually holds now; nothing was written.
 */
export type SaveOutcome =
  | { status: 'saved'; newHash: string; mtime: number }
  | { status: 'conflict'; currentHash: string };

/**
 * Parse the backend DTO into a validated {@link SaveOutcome}. The discriminant is
 * matched exhaustively: an UNRECOGNISED status — or a variant missing its hash —
 * is a hard error, never silently coerced into a conflict-with-empty-hash (which
 * would misfire the conflict banner and lose the write). Exported for unit tests.
 */
export function parseSaveOutcome(dto: SaveOutcomeDto): SaveOutcome {
  switch (dto.status) {
    case 'saved':
      if (dto.new_hash == null) {
        throw new Error('save_doc returned a "saved" outcome without a new hash');
      }
      return { status: 'saved', newHash: dto.new_hash, mtime: dto.mtime ?? 0 };
    case 'conflict':
      if (dto.current_hash == null) {
        throw new Error('save_doc returned a "conflict" outcome without the on-disk hash');
      }
      return { status: 'conflict', currentHash: dto.current_hash };
    default:
      throw new Error(`save_doc returned an unrecognised outcome status: "${dto.status}"`);
  }
}

/**
 * Save `content` to `path` under a hash guard: the write lands only if the file
 * still hashes to `expectedHash` (or `opts.force` overrides the guard — the
 * user's explicit "overwrite the on-disk changes"). A mismatch returns a
 * `conflict` WITHOUT writing, upholding "never silently rewrite files". Throws
 * only on a hard backend error (containment, IO) or an unparseable outcome.
 * `.md`/`.markdown` only.
 */
export async function saveDoc(
  path: string,
  content: string,
  expectedHash: string,
  opts: { force?: boolean } = {},
): Promise<SaveOutcome> {
  const result = await readerBackend().workspace.saveDoc(path, content, expectedHash, opts.force ?? false);
  if (result.status === 'error') throw new Error(result.error);
  return parseSaveOutcome(result.data);
}

/**
 * Open the legacy native folder picker; resolves to the chosen (canonical) path
 * or `null` when cancelled. This never grants authority. Product flows use
 * `pickAndAddLibrarySource`, which returns an opaque source ID.
 */
export async function pickWorkspaceFolder(): Promise<string | null> {
  const result = await readerBackend().workspace.pickWorkspaceFolder();
  if (result.status === 'error') throw new Error(result.error);
  return result.data;
}

/** Subscribe to debounced filesystem changes. Returns an unlisten function. */
export function onFsChanged(handler: (change: FsChanged) => void): Promise<() => void> {
  return readerBackend().workspace.onFsChanged(handler);
}

/** Subscribe to runtime watcher failures. Returns an unlisten function. */
export function onWatchError(handler: (error: WatchError) => void): Promise<() => void> {
  return readerBackend().workspace.onWatchError(handler);
}

// --------------------------------------------------------------------------
// search + related
// --------------------------------------------------------------------------

/** Default hits per phase / related list length. */
const SEARCH_LIMIT = 20;
const RELATED_LIMIT = 12;

/** Normalise a wire search hit (never-null numbers) into the palette's shape. */
function normalizeHit(dto: SearchHitDto & { device_document_id?: string | null; source_kind?: string | null }): SearchHit {
  return {
    chunkId: dto.chunk_id,
    ...(dto.device_document_id != null ? { deviceDocumentId: dto.device_document_id } : {}),
    ...(dto.source_kind === 'local' || dto.source_kind === 'provider' || dto.source_kind === 'capture' || dto.source_kind === 'device'
      ? { sourceKind: dto.source_kind } : {}),
    ...(dto.source_id != null ? { sourceId: dto.source_id } : {}),
    ...(dto.revision != null ? { revision: dto.revision } : {}),
    documentId: dto.document_id,
    path: dto.abs_path,
    filePath: dto.file_path,
    name: dto.name,
    headingPath: dto.heading_path,
    startLine: dto.start_line,
    endLine: dto.end_line,
    score: dto.score ?? 0,
    rrfScore: dto.rrf_score ?? 0,
    boosts: {
      title: dto.boosts.title ?? 0,
      path: dto.boosts.path ?? 0,
      recency: dto.boosts.recency ?? 0,
      graph: dto.boosts.graph ?? 0,
    },
    lexRank: dto.lex_rank,
    semRank: dto.sem_rank,
    snippet: dto.snippet,
    mtime: dto.mtime ?? 0,
    explain: dto.explain
      ? {
          lexRank: dto.explain.lex_rank,
          semRank: dto.explain.sem_rank,
          rrfScore: dto.explain.rrf_score ?? 0,
          boosts: {
            title: dto.explain.boosts.title ?? 0,
            path: dto.explain.boosts.path ?? 0,
            recency: dto.explain.boosts.recency ?? 0,
            graph: dto.explain.boosts.graph ?? 0,
          },
          finalScore: dto.explain.final_score ?? 0,
          indexVersion: dto.explain.index_version,
          embeddingModel: dto.explain.embedding_model,
        }
      : null,
  };
}

/** Normalise a wire related doc into the palette/rail shape. */
function normalizeRelated(dto: RelatedDocDto): RelatedHit {
  return {
    documentId: dto.document_id,
    sourceId: dto.source_id ?? undefined,
    revision: dto.revision ?? undefined,
    line: dto.line ?? undefined,
    path: dto.abs_path,
    filePath: dto.file_path,
    name: dto.name,
    score: dto.score ?? 0,
    graphDirection: dto.graph_direction === 'outgoing' || dto.graph_direction === 'incoming' || dto.graph_direction === 'nearby' ? dto.graph_direction : undefined,
    deviceDocumentId: dto.device_document_id ?? undefined,
    ...(dto.source_kind === 'local' || dto.source_kind === 'provider' || dto.source_kind === 'capture' || dto.source_kind === 'device' ? { sourceKind: dto.source_kind } : {}),
    components: {
      graph: dto.components.graph ?? 0,
      concept: dto.components.concept ?? 0,
      vector: dto.components.vector,
      sharedTags: dto.components.shared_tags ?? undefined,
    },
  };
}

/**
 * Intent-selected search (legacy hybrid when intent is omitted). Best streams
 * two phases: `onPhase('lexical', …)` fires
 * as the FTS batch lands, `onPhase('final', …)` as the hybrid pass merges in
 * (the resolved value is the final outcome — authoritative for a mocked IPC that
 * need not drive the channel). Throws with the schema-derived filter-error text
 * on a bad typed query.
 *
 * The resolved {@link SearchOutcome} carries `semanticUnavailable`: non-null when
 * the vector arm was wanted and could not run. Best preserves lexical fallback
 * with that notice; pure Semantic suppresses lexical previews and fallback hits.
 */
export async function search(
  query: string,
  opts: { lexicalOnly: boolean; explain: boolean; intent?: SearchIntent; providerSources?: readonly string[] },
  onPhase: (phase: 'lexical' | 'final', hits: SearchHit[]) => void,
  currentGeneration?: () => number,
): Promise<SearchOutcome> {
  const onBatch = (batch: SearchBatch) => {
    // Suspenders (the belt is the palette's workspaceKey): a batch tagged with a
    // generation OLDER than the current watcher's belongs to a superseded
    // workspace — drop it, so a switch that interleaved one stale batch onto the
    // channel (the backend send is not under the epoch lock) never renders.
    if (currentGeneration && batch.generation < currentGeneration()) return;
    const phase = batch.phase === 'lexical' ? 'lexical' : 'final';
    if (opts.intent === 'semantic' && phase === 'lexical') return;
    const hits = batch.hits.map(normalizeHit);
    onPhase(phase, opts.intent === 'semantic' ? hits.filter((hit) => hit.semRank !== null) : hits);
  };
  const result = await readerBackend().search.search(
    query,
    {
      limit: SEARCH_LIMIT,
      lexical_only: opts.lexicalOnly,
      explain: opts.explain,
      ...(opts.intent !== undefined ? { intent: opts.intent } : {}),
      ...(opts.providerSources?.length ? { provider_sources: [...opts.providerSources] } : {}),
    },
    onBatch,
  );
  if (result.status === 'error') {
    if (result.error === 'scope_missing') throw new SearchScopeMissingError();
    throw new Error(result.error);
  }
  let hits = result.data.hits.map(normalizeHit);
  if (opts.intent === 'semantic') {
    hits = result.data.semantic_unavailable ? [] : hits.filter((hit) => hit.semRank !== null);
  }
  return { hits, semanticUnavailable: result.data.semantic_unavailable, coverage: normalizeSearchCoverage(result.data.coverage) };
}

/** Documents related to `docPath` (absolute), unified graph+concept+vector. */
export async function related(docPath: string, limit = RELATED_LIMIT): Promise<RelatedOutcome> {
  const result = await readerBackend().search.related(docPath, limit);
  if (result.status === 'error') throw new Error(result.error);
  return normalizeRelatedOutcome(result.data);
}
function normalizeRelatedOutcome(result: RelatedOutcomeDto): RelatedOutcome {
  const a = result.availability;
  return {
    hits: result.hits.map(normalizeRelated),
    availability: {
      vector: a.vector,
      anchorChunks: a.anchor_chunks,
      anchorVectors: a.anchor_vectors,
      legacyConcepts: a.legacy_concepts,
      legacyGraphEntities: a.legacy_graph_entities,
    },
  };
}

/**
 * Build the palette/rail transport, backed by the Tauri command surface.
 * `currentGeneration` returns the generation the CURRENT watcher was armed
 * under; streamed search batches from an older generation are discarded.
 */
export function makeSearchAdapter(currentGeneration: () => number, providerSources: () => readonly string[] = () => []): SearchAdapter {
  return {
    search: (query, opts, onPhase) => search(query, { ...opts, providerSources: providerSources() }, onPhase, currentGeneration),
    related: (docPath) => related(docPath),
    relatedReference: async reference => normalizeRelatedOutcome(await unwrapNative(readerBackend().search.readerRelatedRequest({
      source_id: reference.sourceId, path: reference.path, revision: reference.revision,
      document_id: reference.deviceDocumentId ?? null,
    }))),
  };
}
