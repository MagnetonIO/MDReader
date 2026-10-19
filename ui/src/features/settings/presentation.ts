import { SettingsCommandError, type LibrarySourceDto } from '../../platform/settings.js';
import type { ImportState } from '../../platform/fs.js';
import type { WatcherStatus } from '../../stores/sources.js';

export function readableBytes(bytes: number): string {
  if (bytes % 1024 === 0) return `${bytes / 1024} KiB`;
  return `${bytes.toLocaleString()} bytes`;
}

export function errorMessage(cause: unknown): string {
  if (!(cause instanceof SettingsCommandError)) {
    return cause instanceof Error ? cause.message : 'The request could not be completed.';
  }
  const detail = cause.detail;
  if (!detail || typeof detail !== 'object' || !('kind' in detail)) {
    return 'The request could not be completed.';
  }
  switch (detail.kind) {
    case 'active-source':
      return 'The active source cannot be forgotten. Activate another source first.';
    case 'duplicate-root':
      return 'That folder is already in the library.';
    case 'corrupt-registry':
      return 'The saved library could not be read. Nothing was changed.';
    case 'unsupported-version':
      return 'This library was written by an unsupported MDReader version.';
    case 'invalid-root':
    case 'persist-failed':
    case 'cleanup-failed':
    case 'needs-native-repick':
      return 'MDReader could not finish the local library operation. Try again.';
    case 'corrupt-settings':
      return 'The local diagnostics settings could not be read.';
    case 'invalid-event':
      return 'The diagnostic event was rejected.';
    case 'write-failed':
      return 'MDReader could not write the local diagnostic log.';
    case 'export-failed':
      return 'MDReader could not export the local diagnostic bundle.';
    case 'clear-failed':
      return 'MDReader could not clear the local diagnostic logs.';
    case 'reveal-failed':
      return 'MDReader could not reveal the local diagnostics folder.';
    case 'confirmation-required':
      return 'Confirm before clearing diagnostic logs.';
    case 'source-not-found':
      return 'That source is no longer part of the library.';
    case 'source-unavailable':
      return 'That source is unavailable. Re-add the folder if it moved.';
    case 'grant-mismatch':
      return 'The source did not become active, so its restore preference was not changed.';
    case 'open-failed':
      return 'MDReader could not open that source.';
    case 'invalid-document-path':
      return 'That document is outside the selected folder or is no longer available.';
    case 'read-only-source':
      return 'That source is read-only.';
    case 'offline-migration-required':
      return 'This library needs to be imported before it can be used here. Open Backup & storage to import it into a separate library.';
    default:
      return 'The request could not be completed.';
  }
}

export function sourceState(source: LibrarySourceDto): { label: string; unavailable: boolean } {
  switch (source.state.kind) {
    case 'available':
      return {
    label: 'Available',
    unavailable: false,
  };
    case 'missing':
      return {
    label: 'Folder not found',
    unavailable: true,
  };
    case 'unreadable':
      return {
    label: 'Folder cannot be read',
    unavailable: true,
  };
    case 'not-directory':
      return {
    label: 'No longer a folder',
    unavailable: true,
  };
  }
}

export function activityLabel(watcher: WatcherStatus, importState: ImportState | null): string {
  if (importState?.phase === 'loading') return 'Loading local index model';
  if (importState?.phase === 'importing') {
    return `Indexing ${importState.filesDone} of ${importState.filesTotal} files`;
  }
  if (importState?.phase === 'reembedding') return 'Refreshing the local semantic index';
  if (importState?.phase === 'model-unavailable') return 'Readable; semantic index unavailable';
  if (importState?.phase === 'error') return 'Index needs attention';
  if (watcher === 'watching') return 'Watching for local changes';
  if (watcher === 'error') return 'Folder open; live updates unavailable';
  return 'Idle';
}
