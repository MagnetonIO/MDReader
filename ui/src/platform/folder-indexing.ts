import { readerBackend, unwrapNative } from './backend';
import type { PickedLibrarySource } from './settings';

export interface FolderIndexStatus {
  sourceId: string; monitorEnabled: boolean;
  phase: 'paused' | 'discovering' | 'indexing' | 'ready' | 'error' | 'unavailable';
  knownDocuments: number; indexedDocuments: number; observedAtMs: number | null; complete: boolean; error: string | null;
}
export interface FolderIndexingBackend {
  state(): Promise<FolderIndexStatus[]>;
  setMonitoring(sourceId: string, enabled: boolean): Promise<FolderIndexStatus[]>;
  pickBroadSource(scope: 'home' | 'disk', confirmed: true): Promise<PickedLibrarySource>;
}
export const folderIndexingBackend: FolderIndexingBackend = {
  // Native serializes `phase` as `String`; keep asserting the renderer's closed
  // projection, exactly as the untyped `invoke<T>` did.
  state: () => unwrapNative(readerBackend().folderIndexing.folderIndexingState()) as Promise<FolderIndexStatus[]>,
  setMonitoring: (sourceId, enabled) =>
    unwrapNative(readerBackend().folderIndexing.setFolderMonitoring(sourceId, enabled)) as Promise<FolderIndexStatus[]>,
  pickBroadSource: (scope, confirmed) => unwrapNative(readerBackend().folderIndexing.pickBroadLibrarySource(scope, confirmed)),
};
