/**
 * Thin, ergonomic wrappers over the generated Tauri command surface
 * (`../bindings`). The generated `Result`-returning commands hand back a
 * `{ status: 'ok' | 'error' }` union; these helpers unwrap that union, throwing
 * on error, so the stores and components consume plain promises. This is the one
 * place the webview crosses the fs IPC boundary — swapped for the real engine
 * transport in P3.
 */

import { commands, events, type FileEntry, type DocPayload, type FsChanged } from '../bindings';

export type { FileEntry, DocPayload, FsChanged };

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
}

function normalizeFile(entry: FileEntry): WorkspaceFile {
  return {
    path: entry.path,
    name: entry.name,
    dir: entry.dir,
    mtime: entry.mtime ?? 0,
    size: entry.size ?? 0,
  };
}

/** List the markdown files under `root`. Throws on backend error. */
export async function listWorkspaceFiles(root: string): Promise<WorkspaceFile[]> {
  const result = await commands.listWorkspaceFiles(root);
  if (result.status === 'error') throw new Error(result.error);
  return result.data.map(normalizeFile);
}

/** Read a single document. Throws on backend error. */
export async function readDoc(path: string): Promise<Doc> {
  const result = await commands.readDoc(path);
  if (result.status === 'error') throw new Error(result.error);
  const payload: DocPayload = result.data;
  return { content: payload.content, mtime: payload.mtime ?? 0, hash: payload.hash };
}

/** Open the native folder picker; resolves to the chosen path or `null`. */
export function pickWorkspaceFolder(): Promise<string | null> {
  return commands.pickWorkspaceFolder();
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
