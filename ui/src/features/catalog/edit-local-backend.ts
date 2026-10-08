import { invoke } from '@tauri-apps/api/core';
import type { CanonicalReaderReference } from '../reader/contracts';
import { CatalogError } from './backend';

export interface LocalEditSelection {
  sourceId: string;
  librarySourceId: string;
  root: string;
  path: string;
  relativePath: string;
  revision: string;
}
/** Resolve an existing grant only. Workspace activation remains behind the
 * real unsaved-editor guard and the resolver is repeated after that step. */
export async function resolveLocalEdit(reference: CanonicalReaderReference): Promise<LocalEditSelection> {
  if (reference.documentId || !/^[a-f0-9]{64}$/.test(reference.sourceId)) throw new CatalogError('unauthorized');
  try {
    const value = await invoke<LocalEditSelection & { operation: 'edit_local' }>('reader_catalog', {
      request: { operation: 'edit_local', sourceId: reference.sourceId, path: reference.path, revision: reference.revision },
    });
    if (value.operation !== 'edit_local' || value.sourceId !== reference.sourceId || value.revision !== reference.revision
      || value.relativePath !== reference.path || typeof value.librarySourceId !== 'string' || !value.librarySourceId
      || typeof value.root !== 'string' || !value.root || typeof value.path !== 'string' || !value.path) {
      throw new CatalogError('stale');
    }
    return value;
  } catch (cause) {
    if (cause instanceof CatalogError) throw cause;
    const kind = cause && typeof cause === 'object' && 'kind' in cause ? String(cause.kind) : 'unavailable';
    throw new CatalogError(kind);
  }
}
