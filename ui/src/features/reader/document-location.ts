import { resolveLocalEdit } from '../catalog/edit-local-backend';
import { listLibrarySources } from '../../platform/settings';
import { nativeProvidersBackend } from '../../platform/providers';
import type { CanonicalReaderReference } from './contracts';

export interface DocumentLocation { file: string; source: string; path?: string; location?: string; locationLabel?: string }
export type DocumentKind = 'local' | 'provider' | 'capture' | 'device';
export type LocationResolver = (reference: CanonicalReaderReference, kind: DocumentKind, current: () => boolean) => Promise<DocumentLocation | null>;
const dependencies = { local: resolveLocalEdit, libraries: listLibrarySources, providers: nativeProvidersBackend };

/** Local paths require exact native membership proof; provider metadata never becomes a disk path. */
export async function resolveDocumentLocation(reference: CanonicalReaderReference, kind: DocumentKind,
  current: () => boolean, backend = dependencies): Promise<DocumentLocation | null> {
  if (kind === 'local' && !reference.documentId && !reference.sourceId.startsWith('provider:')) {
    const exact = await backend.local(reference);
    if (!current()) return null;
    const library = await backend.libraries();
    if (!current()) return null;
    const members = library.sources.filter(member => member.id === exact.librarySourceId && member.path === exact.root && member.state.kind === 'available');
    return { file: reference.path.split('/').at(-1)!, source: members.length === 1 ? members[0]!.name : 'Local folder', path: exact.path };
  }
  if (kind !== 'provider' || !reference.sourceId.startsWith('provider:')) return null;
  const snapshot = await backend.providers.state();
  if (!current()) return null;
  const sources = snapshot.sources.filter(source => source.readerSourceId === reference.sourceId && source.enabled);
  if (sources.length !== 1) return null;
  const source = sources[0]!;
  const provider = source.kind === 'google_drive' ? 'Google Drive' : 'Dropbox';
  const details: DocumentLocation = { file: reference.title && reference.title !== reference.path ? reference.title : 'Cloud document', source: `${provider} · ${source.name}` };
  const scope = source.selectedScope;
  if (source.kind === 'google_drive') {
    // Drive displayPath contains opaque folder/file IDs, not a friendly breadcrumb.
    if (scope.kind === 'drive_folder' || scope.kind === 'drive_readonly_folder')
      details.location = scope.kind === 'drive_readonly_folder' && scope.accountWide ? 'Accessible Drive files'
        : scope.folderId === 'root' ? 'My Drive' : 'Selected Drive folder';
  } else if (scope.kind === 'scoped_root' && scope.path) { details.location = scope.path; details.locationLabel = 'Dropbox folder'; }
  let offset = 0;
  while (offset <= 16384) {
    let page;
    try { page = await backend.providers.documents(source.sourceId, offset, 256); }
    catch { return current() ? details : null; }
    if (!current()) return null;
    if (page.offset !== offset) return details;
    const matches = page.documents.filter(document => document.admitted && document.projection?.readerSourceId === reference.sourceId
      && document.projection.path === reference.path && document.projection.revision === reference.revision);
    if (matches.length > 1) return details;
    if (matches.length === 1) {
      details.file = matches[0]!.name;
      if (source.kind === 'dropbox') { details.location = matches[0]!.displayPath; details.locationLabel = 'Dropbox path'; }
      return details;
    }
    if (page.nextOffset === null || !Number.isSafeInteger(page.nextOffset) || page.nextOffset <= offset || page.nextOffset > 16384) break;
    offset = page.nextOffset;
  }
  return details;
}
