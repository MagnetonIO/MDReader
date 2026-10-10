import type { ProviderSource } from '../../platform/providers';
import { ProviderIcon } from './ProviderIcon';

/** Describes the saved grant and metadata observation, never account-wide/body coverage. */
export function ProviderSourceSummary({ source, allowEditing }: { source: ProviderSource; allowEditing: boolean }) {
  const listing = source.metadataListing;
  const status = !source.enabled ? 'Paused' : source.health === 'ready' && source.selectedScope.kind !== 'imported_legacy_broad'
    ? 'Connected' : source.health === 'reconnect_required' ? 'Reconnect needed' : 'Connection unavailable';
  const count = listing && listing.completeness !== 'never_observed'
    ? `${listing.knownDocuments} Markdown file${listing.knownDocuments === 1 ? '' : 's'}${listing.completeness === 'partial' ? ' found so far' : ' in this selection'}` : listing ? 'File list not loaded yet' : 'Refresh to check the file list';
  const observed = listing?.observedAtMs;
  const checked = observed !== null && observed !== undefined ? new Date(observed) : null;
  return <>
    <h3 className="provider-source-list__title"><ProviderIcon provider={source.kind} />{source.name}</h3>
    <p>{source.kind === 'google_drive' ? 'Google Drive' : 'Dropbox'} · {status}</p>
    <dl className="provider-source-summary">
      <div><dt>Folder / selection</dt><dd>{scopeLabel(source)}</dd></div>
      <div><dt>Editing</dt><dd>{allowEditing && source.capabilities.guardedWrite
        ? 'Allowed for this connection' : 'Read only for this connection'}</dd></div>
      <div><dt>Files</dt><dd>{count}{listing?.refreshPending ? ' · More files remain to be checked' : ''}</dd></div>
      {checked && Number.isFinite(checked.getTime()) ? <div><dt>Last checked</dt>
        <dd><time dateTime={checked.toISOString()}>{checked.toLocaleString()}</time></dd></div> : null}
    </dl>
  </>;
}

function scopeLabel(source: ProviderSource): string {
  const scope = source.selectedScope;
  switch (scope.kind) {
    case 'exact_documents': return `${scope.count} explicitly selected file${scope.count === 1 ? '' : 's'}`;
    case 'scoped_root': return `${scope.path ?? 'Selected Dropbox folder'} · ${scope.recursive ? 'Includes subfolders' : 'Direct files only'}`;
    case 'drive_folder': return `${scope.folderId === 'root' ? 'My Drive' : 'Selected Drive folder'} · ${scope.recursive ? 'Includes subfolders' : 'Direct files only'}`;
    case 'drive_readonly_folder': return scope.accountWide ? 'My Drive · Includes subfolders' : 'Selected Drive folder · Direct files only';
    case 'imported_legacy_broad': return 'Choose a new folder to reconnect this imported connection';
  }
}
