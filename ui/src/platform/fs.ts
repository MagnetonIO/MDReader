/**
 * Thin, ergonomic wrappers over the generated Tauri command surface
 * (`../bindings`). The generated `Result`-returning commands hand back a
 * `{ status: 'ok' | 'error' }` union; these helpers unwrap that union, throwing
 * on error, so the stores and components consume plain promises. This is the one
 * place the webview crosses the fs/engine IPC boundary.
 */

import { Channel } from '@tauri-apps/api/core';

import {
  commands,
  events,
  type FileEntry,
  type DocPayload,
  type FsChanged,
  type WatchError,
  type ImportProgress,
  type WikiResolution as BindingWikiResolution,
} from '../bindings';

export type { FileEntry, DocPayload, FsChanged, WatchError };

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

/** Import phase for the StatusBar indicator. */
export type ImportPhase = 'idle' | 'importing' | 'done' | 'error';

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
      phase === 'importing' || phase === 'done' || phase === 'error' ? phase : 'idle',
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
 * Open the native folder picker; resolves to the chosen (canonical) path or
 * `null` when cancelled. Does NOT grant the folder — the grant is adopted only
 * when the caller commits via {@link watchWorkspace}. Throws on backend error.
 */
export async function pickWorkspaceFolder(): Promise<string | null> {
  const result = await commands.pickWorkspaceFolder();
  if (result.status === 'error') throw new Error(result.error);
  return result.data;
}

/** Begin watching `root`. Throws on backend error. */
export async function watchWorkspace(root: string): Promise<void> {
  const result = await commands.watchWorkspace(root);
  if (result.status === 'error') throw new Error(result.error);
}

/** Subscribe to debounced filesystem changes. Returns an unlisten function. */
export function onFsChanged(handler: (change: FsChanged) => void): Promise<() => void> {
  return events.mdreaderFsChanged.listen((event) => handler(event.payload));
}

/** Subscribe to runtime watcher failures. Returns an unlisten function. */
export function onWatchError(handler: (error: WatchError) => void): Promise<() => void> {
  return events.mdreaderWatchError.listen((event) => handler(event.payload));
}
