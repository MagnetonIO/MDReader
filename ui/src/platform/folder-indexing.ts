import { invoke } from '@tauri-apps/api/core';
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
  state: () => invoke('folder_indexing_state'),
  setMonitoring: (sourceId, enabled) => invoke('set_folder_monitoring', { sourceId, enabled }),
  pickBroadSource: (scope, confirmed) => invoke('pick_broad_library_source', { scope, confirmed }),
};
