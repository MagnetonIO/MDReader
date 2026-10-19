/**
 * The renderer's native backend seam. Every IPC call the renderer makes goes
 * through a {@link ReaderBackend}; only `tauri-backend.ts` may reach the
 * generated bindings or `@tauri-apps/api/core` (enforced by ESLint).
 *
 * Methods mirror the generated Specta commands one-for-one (same names, same
 * argument order, same `{ status: 'ok' | 'error' }` replies for commands whose
 * Rust handler returns `Result`), grouped by domain. Payload types therefore
 * come straight from the generated bindings. The exceptions are the streaming
 * commands, which take plain callbacks instead of a Tauri `Channel`, and the
 * fs event subscriptions.
 *
 * Plain modules and zustand stores call {@link readerBackend}; React code may
 * use {@link useReaderBackend}, which reads the provider mounted in `main.tsx`.
 * Tests can swap the whole backend with {@link setReaderBackend}.
 */

import { createBackendContext } from '@mdreader/ui/platform';
import type {
  commands, FsChanged, ImportProgress, SearchBatch, SearchQuery, ShowSettings, WatchError,
} from '../bindings';
import { tauriBackend } from './tauri-backend';

type Commands = typeof commands;
type Reply<K extends keyof Commands> = ReturnType<Commands[K]>;
type Unlisten = () => void;

/** Capabilities of the mobile share/export sheet (not in the generated bindings). */
export interface TransferCapabilities { export: boolean; share: boolean; readingLinks: boolean }

export interface ReaderBackend {
  app: Pick<Commands, 'ping'> & {
    /** The native menu's "Settings…" request. */
    onShowSettings(handler: (payload: ShowSettings) => void): Promise<Unlisten>;
  };
  updates: Pick<Commands, 'updaterAvailable' | 'checkForUpdate' | 'installUpdate'>;
  account: Pick<Commands, 'readerAccount'>;
  profile: Pick<Commands,
    | 'nativeProfileStartup' | 'nativeProfileBackup' | 'nativeProfileMigration' | 'nativeProfileImportedHistory'>;
  persistence: Pick<Commands,
    | 'nativeProfilePersistenceHost' | 'nativeProfilePersistenceLoad' | 'nativeProfilePersistenceRead'
    | 'nativeProfilePersistenceWrite' | 'nativeProfilePersistenceFlush' | 'nativeProfileEditorSubmit'
    | 'nativeEditorDrafts'>;
  workspace: Pick<Commands,
    | 'listWorkspaceFiles' | 'importStatus' | 'resolveWikilink' | 'readDoc' | 'saveDoc'
    | 'pickWorkspaceFolder' | 'readerWorkspaceAsset'> & {
    /** `start_import`, streaming progress to `onProgress` (best-effort). */
    startImport(root: string, onProgress: (progress: ImportProgress) => void): Reply<'startImport'>;
    onFsChanged(handler: (payload: FsChanged) => void): Promise<Unlisten>;
    onWatchError(handler: (payload: WatchError) => void): Promise<Unlisten>;
  };
  search: Pick<Commands, 'related' | 'readerRelatedRequest' | 'readerEmbeddingPolicy' | 'setReaderEmbeddingPolicy'> & {
    /** `search`, streaming lexical/final batches to `onBatch`. */
    search(query: string, options: SearchQuery, onBatch: (batch: SearchBatch) => void): Reply<'search'>;
  };
  library: Pick<Commands,
    | 'listLibrarySources' | 'pickAndAddLibrarySource' | 'reconcileLibraryRestore' | 'watchLibrarySource'
    | 'grantLibrarySource' | 'markLibrarySourceActive' | 'forgetLibrarySource' | 'closeAndForgetLibrarySource'>;
  diagnostics: Pick<Commands,
    | 'diagnosticsInfo' | 'setDiagnosticsLevel' | 'recordDiagnosticEvent' | 'revealDiagnostics'
    | 'exportDiagnostics' | 'clearDiagnostics'>;
  folderIndexing: Pick<Commands, 'folderIndexingState' | 'setFolderMonitoring' | 'pickBroadLibrarySource'>;
  agents: Pick<Commands,
    | 'agentIntegrationsState' | 'prepareAgentIntegration' | 'applyAgentIntegration' | 'uninstallAgentIntegration'
    | 'agentCaptureState' | 'setAgentCapture' | 'agentCaptureBackfill'
    | 'agentSetupState' | 'agentSetupInstall' | 'agentSetupSetMcp' | 'agentSetupSetPermissions' | 'agentSetupUninstall'
    | 'agentProjectsList' | 'agentProjectSessions' | 'agentProjectDocuments'
    | 'agentPlansSourceStatus' | 'enableAgentPlansSource' | 'disableAgentPlansSource'>;
  catalog: Pick<Commands, 'readerCatalog'>;
  device: Pick<Commands, 'readerDevice'>;
  providers: Pick<Commands,
    | 'nativeProviderStatus' | 'nativeProviderConnect' | 'nativeProviderReconnect' | 'nativeProviderReconcile'
    | 'nativeProviderRequest'>;
  documentTransfer: Pick<Commands, 'readerExportDocument' | 'readerExportDraft'>;
  reviews: Pick<Commands, 'readerArchivedReviews'>;
  externalLinks: Pick<Commands, 'readerOpenExternal'>;
  mobile: Pick<Commands,
    | 'mobileReaderList' | 'mobileReaderRead' | 'mobileReaderAssets' | 'mobileReaderImportIncoming'
    | 'mobileReaderTakeReadingLinks' | 'mobileReaderTransfer' | 'mobileProviderReadingState' | 'mobileProviderTransfer'
    | 'mobileReaderImport' | 'mobileReaderDiscardImport' | 'mobileReaderPin' | 'mobileReaderPosition' | 'mobileReaderRebuild'
    | 'mobileSearchSettings' | 'mobileSearchSetEnabled' | 'mobileSearchQuery'> & {
    /** `mobile_reader_transfer_capabilities` (registered natively, absent from the bindings). */
    mobileReaderTransferCapabilities(): Promise<TransferCapabilities>;
  };
}

export type NativeResult<T, E> = { status: 'ok'; data: T } | { status: 'error'; error: E };

/**
 * Resolve a `Result` reply to its data, rejecting with the native error payload
 * unchanged — exactly what a raw `invoke` of the same command rejects with.
 */
export async function unwrapNative<T, E>(pending: Promise<NativeResult<T, E>>): Promise<T> {
  const result = await pending;
  if (result.status === 'error') throw result.error;
  return result.data;
}

let current: ReaderBackend = tauriBackend;

/** The active backend, for code outside React (modules, stores, adapters). */
export function readerBackend(): ReaderBackend {
  return current;
}

/** Replace the active backend; returns a function restoring the previous one. */
export function setReaderBackend(next: ReaderBackend): () => void {
  const previous = current;
  current = next;
  return () => { current = previous; };
}

export const { BackendProvider: ReaderBackendProvider, useBackend: useReaderBackend } =
  createBackendContext<ReaderBackend>(readerBackend);
