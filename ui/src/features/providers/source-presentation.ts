import type { NativeProviderKind, ProviderDisconnect, ProviderSource } from '../../platform/providers';

export type SourceStatus = 'connected' | 'paused' | 'reconnect' | 'unavailable';

export const sourceStatusLabel: Record<SourceStatus, string> = {
  connected: 'Connected', paused: 'Paused', reconnect: 'Reconnect needed', unavailable: 'Unavailable',
};

export function providerName(kind: NativeProviderKind | string): string {
  return kind === 'google_drive' ? 'Google Drive' : kind === 'dropbox' ? 'Dropbox' : 'Cloud';
}

/** An imported broad connection cannot be used until a new folder is chosen. */
export function sourceStatus(source: ProviderSource): SourceStatus {
  if (!source.enabled) return 'paused';
  if (source.health === 'reconnect_required' || source.selectedScope.kind === 'imported_legacy_broad') return 'reconnect';
  return source.health === 'ready' ? 'connected' : 'unavailable';
}

/** Says exactly what was removed. Unless the provider confirmed revocation, point to its own settings. */
export function disconnectNotice(source: ProviderSource, result: ProviderDisconnect): string {
  const provider = providerName(source.kind);
  if (result.credential === 'shared') {
    return `${source.name} (${provider}) was disconnected and its downloaded copies were removed. Its saved sign-in is still used by another source, so it was kept.`;
  }
  const removed = `${source.name} (${provider}) was disconnected. Its downloaded copies and saved sign-in were removed.`;
  const remove = `you can remove MDReader from your ${provider} account's connected apps.`;
  switch (result.revocation) {
    case 'revoked': return removed;
    case 'failed': return `${removed} ${provider} did not confirm that MDReader's access was revoked; ${remove}`;
    case 'not_attempted': return `${removed} MDReader could not ask ${provider} to revoke its access; ${remove}`;
  }
}

export function canBrowseSource(source: ProviderSource): boolean {
  return source.enabled && source.selectedScope.kind !== 'imported_legacy_broad';
}

/** Describes the saved grant and metadata observation, never account-wide/body coverage. */
export function sourceDetails(source: ProviderSource, allowEditing: boolean, now = new Date()): string[] {
  const parts = scopeParts(source);
  parts.push(allowEditing && source.capabilities.guardedWrite ? 'can edit' : 'read-only');
  const listing = source.metadataListing;
  if (!listing || listing.completeness === 'never_observed') parts.push('file list not loaded');
  else {
    const files = `${listing.knownDocuments} file${listing.knownDocuments === 1 ? '' : 's'}`;
    parts.push(listing.completeness === 'partial' ? `${files} so far` : files);
  }
  if (listing?.refreshPending) parts.push('more files pending');
  const checked = checkedTime(listing?.observedAtMs ?? null, now);
  if (checked) parts.push(`checked ${checked}`);
  return parts;
}

export function checkedDate(observedAtMs: number | null | undefined): Date | null {
  if (observedAtMs === null || observedAtMs === undefined) return null;
  const value = new Date(observedAtMs);
  return Number.isFinite(value.getTime()) ? value : null;
}

function checkedTime(observedAtMs: number | null, now: Date): string | null {
  const value = checkedDate(observedAtMs);
  if (!value) return null;
  return value.toDateString() === now.toDateString()
    ? value.toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' })
    : value.toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
}

/** Opaque provider folder IDs never reach the display. */
function scopeParts(source: ProviderSource): string[] {
  const scope = source.selectedScope;
  const depth = (recursive: boolean) => recursive ? 'subfolders' : 'no subfolders';
  switch (scope.kind) {
    case 'exact_documents': return [`${scope.count} chosen file${scope.count === 1 ? '' : 's'}`];
    case 'scoped_root': return [scope.path ?? 'Dropbox folder', depth(scope.recursive)];
    case 'drive_folder': return [scope.folderId === 'root' ? 'My Drive' : 'Drive folder', depth(scope.recursive)];
    case 'drive_readonly_folder': return scope.accountWide ? ['Accessible Drive files'] : ['Drive folder', depth(false)];
    case 'imported_legacy_broad': return ['Imported connection — choose a new folder'];
  }
}
