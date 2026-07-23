/**
 * Thin, ergonomic wrappers over the generated Tauri command surface
 * (`../bindings`). The generated `Result`-returning commands hand back a
 * `{ status: 'ok' | 'error' }` union; these helpers unwrap that union, throwing
 * on error, so the stores and components consume plain promises. This is the one
 * place the webview crosses the fs/engine IPC boundary.
 */

import { Channel } from '@tauri-apps/api/core';
import type { RelatedHit, SearchAdapter, SearchHit } from '@mdreader/ui';

import {
  commands,
  events,
  type FileEntry,
  type DocPayload,
  type SaveOutcome as SaveOutcomeDto,
  type FsChanged,
  type WatchError,
  type ImportProgress,
  type WikiResolution as BindingWikiResolution,
  type SearchHitDto,
  type SearchBatch,
  type RelatedDocDto,
} from '../bindings';

export type { FileEntry, DocPayload, FsChanged, WatchError };
export type { SearchHit, RelatedHit } from '@mdreader/ui';

/** A `blake3`/`f64`-normalised document payload (never-null numbers). */
export interface Doc {
  content: string;
  mtime: number;
  hash: string;
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

/** The canonical grant plus the generation its watcher was armed under. */
export interface WorkspaceGrant {
  root: string;
  generation: number;
}

/** Import phase for the StatusBar indicator. `loading` covers the one-time
 *  embedding-model construction/download that precedes the first import. */
export type ImportPhase = 'idle' | 'loading' | 'importing' | 'done' | 'error';

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
      phase === 'loading' || phase === 'importing' || phase === 'done' || phase === 'error'
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
 * Grant `root` as the active workspace (canonicalized backend-side); every
 * subsequent read/list/watch is confined to it. Resolves to the canonical path.
 * Throws on backend error (e.g. the folder no longer exists).
 */
export async function setWorkspace(root: string): Promise<string> {
  const result = await commands.setWorkspace(root);
  if (result.status === 'error') throw new Error(result.error);
  return result.data;
}

/**
 * List the markdown files under `root`, from the engine index once an import has
 * finished (gitignored files already dropped) or a live filesystem scan before
 * then (gitignored files flagged for dimming). Throws on backend error.
 */
export async function listWorkspaceFiles(root: string): Promise<WorkspaceListing> {
  const result = await commands.listWorkspaceFiles(root);
  if (result.status === 'error') throw new Error(result.error);
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
  const channel = new Channel<ImportProgress>();
  channel.onmessage = (progress) => onProgress(normalizeProgress(progress));
  const result = await commands.startImport(root, channel);
  if (result.status === 'error') throw new Error(result.error);
  return normalizeProgress(result.data);
}

/** The latest import status (for initial render / polling). Throws on error. */
export async function importStatus(): Promise<ImportState> {
  const result = await commands.importStatus();
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
  const result = await commands.resolveWikilink(fromDoc, target, heading ?? null);
  if (result.status === 'error') throw new Error(result.error);
  return normalizeResolution(result.data);
}

/** Read a single document. Throws on backend error. */
export async function readDoc(path: string): Promise<Doc> {
  const result = await commands.readDoc(path);
  if (result.status === 'error') throw new Error(result.error);
  const payload: DocPayload = result.data;
  return { content: payload.content, mtime: payload.mtime ?? 0, hash: payload.hash };
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
  const result = await commands.saveDoc(path, content, expectedHash, opts.force ?? false);
  if (result.status === 'error') throw new Error(result.error);
  return parseSaveOutcome(result.data);
}

/**
 * Open the native folder picker; resolves to the chosen (canonical) path or
 * `null` when cancelled. Does NOT grant the folder — the grant is adopted only
 * when the caller commits via {@link watchWorkspace}. Throws on backend error.
 */
export async function pickWorkspaceFolder(): Promise<string | null> {
  const result = await commands.pickWorkspaceFolder();
  if (result.status === 'error') throw new Error(result.error);
  return result.data;
}

/**
 * The atomic "switch workspace" operation: begin watching `root`, adopt it as
 * the grant (advancing the root generation), and resolve to its canonical path
 * PLUS the generation the watcher was armed under — all in one awaited round-trip
 * (no separate {@link setWorkspace} to sequence). The generation is the reference
 * the frontend filters `fs-changed` / `watch-error` events against. Throws on
 * backend error, leaving the previous workspace fully intact.
 */
export async function watchWorkspace(root: string): Promise<WorkspaceGrant> {
  const result = await commands.watchWorkspace(root);
  if (result.status === 'error') throw new Error(result.error);
  return { root: result.data.root, generation: result.data.generation };
}

/** Subscribe to debounced filesystem changes. Returns an unlisten function. */
export function onFsChanged(handler: (change: FsChanged) => void): Promise<() => void> {
  return events.mdreaderFsChanged.listen((event) => handler(event.payload));
}

/** Subscribe to runtime watcher failures. Returns an unlisten function. */
export function onWatchError(handler: (error: WatchError) => void): Promise<() => void> {
  return events.mdreaderWatchError.listen((event) => handler(event.payload));
}

// --------------------------------------------------------------------------
// search + related
// --------------------------------------------------------------------------

/** Default hits per phase / related list length. */
const SEARCH_LIMIT = 20;
const RELATED_LIMIT = 12;

/** Normalise a wire search hit (never-null numbers) into the palette's shape. */
function normalizeHit(dto: SearchHitDto): SearchHit {
  return {
    chunkId: dto.chunk_id,
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
    path: dto.abs_path,
    filePath: dto.file_path,
    name: dto.name,
    score: dto.score ?? 0,
    components: {
      graph: dto.components.graph ?? 0,
      concept: dto.components.concept ?? 0,
      vector: dto.components.vector ?? 0,
    },
  };
}

/**
 * Hybrid search, streamed in two honest phases: `onPhase('lexical', …)` fires
 * as the FTS batch lands, `onPhase('final', …)` as the hybrid pass merges in
 * (the resolved value is the final list — authoritative for a mocked IPC that
 * need not drive the channel). Throws with the schema-derived filter-error text
 * on a bad typed query.
 */
export async function search(
  query: string,
  opts: { lexicalOnly: boolean; explain: boolean },
  onPhase: (phase: 'lexical' | 'final', hits: SearchHit[]) => void,
  currentGeneration?: () => number,
): Promise<SearchHit[]> {
  const channel = new Channel<SearchBatch>();
  channel.onmessage = (batch) => {
    // Suspenders (the belt is the palette's workspaceKey): a batch tagged with a
    // generation OLDER than the current watcher's belongs to a superseded
    // workspace — drop it, so a switch that interleaved one stale batch onto the
    // channel (the backend send is not under the epoch lock) never renders.
    if (currentGeneration && batch.generation < currentGeneration()) return;
    const phase = batch.phase === 'lexical' ? 'lexical' : 'final';
    onPhase(phase, batch.hits.map(normalizeHit));
  };
  const result = await commands.search(
    query,
    { limit: SEARCH_LIMIT, lexical_only: opts.lexicalOnly, explain: opts.explain },
    channel,
  );
  if (result.status === 'error') throw new Error(result.error);
  return result.data.map(normalizeHit);
}

/** Documents related to `docPath` (absolute), unified graph+concept+vector. */
export async function related(docPath: string, limit = RELATED_LIMIT): Promise<RelatedHit[]> {
  const result = await commands.related(docPath, limit);
  if (result.status === 'error') throw new Error(result.error);
  return result.data.map(normalizeRelated);
}

/**
 * Build the palette/rail transport, backed by the Tauri command surface.
 * `currentGeneration` returns the generation the CURRENT watcher was armed
 * under; streamed search batches from an older generation are discarded.
 */
export function makeSearchAdapter(currentGeneration: () => number): SearchAdapter {
  return {
    search: (query, opts, onPhase) => search(query, opts, onPhase, currentGeneration),
    related: (docPath) => related(docPath),
  };
}
