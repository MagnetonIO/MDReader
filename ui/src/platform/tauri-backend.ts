/**
 * The Tauri implementation of {@link ReaderBackend}: the only renderer module
 * allowed to import the generated bindings or `@tauri-apps/api/core`.
 *
 * Commands are forwarded lazily (looked up on each call) so tests that mock
 * `../bindings` per file keep working, and so a command is never resolved
 * before the renderer actually uses it.
 */

import { Channel, invoke } from '@tauri-apps/api/core';
import {
  commands, events, type ImportProgress, type SearchBatch,
} from '../bindings';
import type { ReaderBackend, TransferCapabilities } from './backend';

type Commands = typeof commands;

function forward<K extends keyof Commands>(...names: K[]): Pick<Commands, NoInfer<K>> {
  return Object.fromEntries(names.map(name => [
    name,
    (...args: unknown[]) => (commands[name] as (...args: unknown[]) => unknown)(...args),
  ])) as unknown as Pick<Commands, K>;
}

export const tauriBackend: ReaderBackend = {
  app: {
    ...forward('ping'),
    onShowSettings: handler => events.mdreaderShowSettings.listen(event => handler(event.payload)),
  },
  updates: forward('updaterAvailable', 'checkForUpdate', 'installUpdate'),
  account: forward('readerAccount'),
  profile: forward(
    'nativeProfileStartup', 'nativeProfileBackup', 'nativeProfileMigration', 'nativeProfileImportedHistory',
  ),
  persistence: forward(
    'nativeProfilePersistenceHost', 'nativeProfilePersistenceLoad', 'nativeProfilePersistenceRead',
    'nativeProfilePersistenceWrite', 'nativeProfilePersistenceFlush', 'nativeProfileEditorSubmit',
    'nativeEditorDrafts',
  ),
  workspace: {
    ...forward(
      'listWorkspaceFiles', 'importStatus', 'resolveWikilink', 'readDoc', 'saveDoc',
      'pickWorkspaceFolder', 'readerWorkspaceAsset',
    ),
    startImport(root, onProgress) {
      const channel = new Channel<ImportProgress>();
      channel.onmessage = onProgress;
      return commands.startImport(root, channel);
    },
    onFsChanged: handler => events.mdreaderFsChanged.listen(event => handler(event.payload)),
    onWatchError: handler => events.mdreaderWatchError.listen(event => handler(event.payload)),
  },
  search: {
    ...forward('related', 'readerRelatedRequest', 'readerEmbeddingPolicy', 'setReaderEmbeddingPolicy'),
    search(query, options, onBatch) {
      const channel = new Channel<SearchBatch>();
      channel.onmessage = onBatch;
      return commands.search(query, options, channel);
    },
  },
  library: forward(
    'listLibrarySources', 'pickAndAddLibrarySource', 'reconcileLibraryRestore', 'watchLibrarySource',
    'grantLibrarySource', 'markLibrarySourceActive', 'forgetLibrarySource', 'closeAndForgetLibrarySource',
  ),
  diagnostics: forward(
    'diagnosticsInfo', 'setDiagnosticsLevel', 'recordDiagnosticEvent', 'revealDiagnostics',
    'exportDiagnostics', 'clearDiagnostics',
  ),
  folderIndexing: forward('folderIndexingState', 'setFolderMonitoring', 'pickBroadLibrarySource'),
  agents: forward(
    'agentIntegrationsState', 'prepareAgentIntegration', 'applyAgentIntegration', 'uninstallAgentIntegration',
    'agentCaptureState', 'setAgentCapture', 'agentCaptureBackfill',
    'agentSetupState', 'agentSetupInstall', 'agentSetupSetMcp', 'agentSetupSetPermissions', 'agentSetupUninstall',
  ),
  agentActivity: {
    ...forward(
      'agentActivityList', 'agentActivityFiles', 'agentActivityRename', 'agentActivityHide', 'agentActivityRevealCwd',
      'agentActivityTrackingState', 'setAgentActivityTracking', 'agentActivityLoadOlder', 'agentActivityForget',
    ),
    onChanged: handler => events.mdreaderAgentActivityChanged.listen(event => handler(event.payload)),
  },
  catalog: forward('readerCatalog'),
  device: forward('readerDevice'),
  providers: forward(
    'nativeProviderStatus', 'nativeProviderConnect', 'nativeProviderReconnect', 'nativeProviderReconcile',
    'nativeProviderRequest',
  ),
  documentTransfer: forward('readerExportDocument', 'readerExportDraft'),
  reviews: forward('readerArchivedReviews'),
  externalLinks: forward('readerOpenExternal'),
  mobile: {
    ...forward(
      'mobileReaderList', 'mobileReaderRead', 'mobileReaderAssets', 'mobileReaderImportIncoming',
      'mobileReaderTakeReadingLinks', 'mobileReaderTransfer', 'mobileProviderReadingState', 'mobileProviderTransfer',
      'mobileReaderImport', 'mobileReaderDiscardImport', 'mobileReaderPin', 'mobileReaderPosition', 'mobileReaderRebuild',
      'mobileSearchSettings', 'mobileSearchSetEnabled', 'mobileSearchQuery',
    ),
    // Registered by the mobile shell but not collected into the Specta
    // bindings, so this is the one hand-typed command.
    mobileReaderTransferCapabilities: () => invoke<TransferCapabilities>('mobile_reader_transfer_capabilities'),
  },
};
