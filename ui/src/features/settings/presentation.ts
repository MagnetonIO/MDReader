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
    case 'client-configuration-missing':
      return 'This MDReader build is missing the provider sign-in configuration.';
    case 'credential-unavailable':
      return 'The operating system credential store is unavailable.';
    case 'credential-missing':
      return 'Sign in again before enabling this source.';
    case 'source-paused':
      return 'Include this source in the Library before reading it.';
    case 'oauth-callback-unavailable':
      return 'MDReader could not open its local sign-in callback. Close another sign-in attempt and try again.';
    case 'oauth-state-mismatch':
    case 'oauth-invalid-callback':
      return 'The provider sign-in response could not be verified. Nothing was connected.';
    case 'oauth-denied':
      return 'The provider sign-in was cancelled or denied.';
    case 'oauth-timeout':
      return 'Provider sign-in timed out. Try again.';
    case 'oauth-token-rejected':
      return 'Sign-in completed, but the provider rejected the token exchange. Check the OAuth app configuration and try again.';
    case 'oauth-offline-grant-missing':
      return 'Sign-in completed, but the provider did not issue an offline access token. Remove MDReader from the provider’s connected apps, then connect again.';
    case 'oauth-transport':
      return 'The provider could not complete sign-in. Try again.';
    case 'provider-rejected':
      return 'Sign-in completed, but the provider rejected the first Library read. Confirm its API is enabled for MDReader’s OAuth project and try again.';
    case 'query-invalid':
      return detail.detail;
    case 'query-failed':
      return `MDReader could not read that source. ${detail.detail}`;
    case 'query-not-found':
      return 'That provider document was not found. Refresh the source and try again.';
    case 'invalid-configuration':
    case 'rollback-failed':
      return 'MDReader could not safely update the cloud source. Nothing was connected.';
    case 'permission-denied':
      return 'That CatDB token does not grant one workspace with query and catalog access.';
    case 'connection-identity-changed':
      return 'The CatDB server identity changed. Disconnect it and verify the server before reconnecting.';
    case 'already-configured':
      return 'Disconnect the current CatDB workspace before connecting another.';
    case 'connection-not-found':
      return 'That CatDB workspace is no longer connected.';
    case 'no-sources-selected':
      return 'Choose at least one permitted CatDB source before querying.';
    case 'transport-unavailable':
    case 'timed-out':
      return 'The CatDB server could not be reached. Check its URL and try again.';
    case 'response-too-large':
    case 'invalid-response':
      return 'The CatDB server returned a response MDReader could not safely use.';
    case 'invalid-request':
      return 'The CatDB connection request was rejected before it left this device.';
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
