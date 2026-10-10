import type { NativeProvidersBackend, ProviderReaderReference } from '../../platform/providers';
import { receipt } from '../reader-history/references';

type Backend = Pick<NativeProvidersBackend, 'state' | 'documents' | 'fetchCurrent'>;
/** Resolve an existing exact creation projection to its native handle, never by filename. */
export async function currentCreatedDocument(target: ProviderReaderReference, backend: Backend,
  active: () => boolean): Promise<ProviderReaderReference | null> {
  const snapshot = await backend.state();
  if (!active()) return null;
  const sources = snapshot.sources.filter(source => source.readerSourceId === target.sourceId
    && source.enabled && source.health === 'ready' && source.capabilities.guardedWrite);
  if (sources.length !== 1) throw new Error('The new document’s connection is unavailable. Reopen it from its source.');
  const source = sources[0]!;
  let offset = 0;
  while (offset <= 16384) {
    const page = await backend.documents(source.sourceId, offset, 256);
    if (!active()) return null;
    if (page.offset !== offset) break;
    const matches = page.documents.filter(document => document.admitted && document.projection?.readerSourceId === target.sourceId
      && document.projection.path === target.path);
    if (matches.length > 1) break;
    if (matches.length === 1) {
      const projection = await backend.fetchCurrent(source.sourceId, matches[0]!.documentId);
      if (!active()) return null;
      if (projection.readerSourceId !== target.sourceId || projection.path !== target.path) break;
      const current = { ...target, revision: projection.revision };
      receipt(current); // Preserve exact native receipt admission before selecting or editing.
      return current;
    }
    if (page.nextOffset === null || page.nextOffset <= offset || page.nextOffset > 16384) break;
    offset = page.nextOffset;
  }
  throw new Error('The new document’s current copy is unavailable. Reopen it from its source.');
}
