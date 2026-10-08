import type { CanonicalReaderReference } from '../reader/contracts';
import type { ReaderEntry, ReaderReference } from './model';
import { parseReference } from './serialization';
export function receipt(reference: CanonicalReaderReference): ReaderReference {
  const parsed = parseReference(reference.documentId
    ? { kind: 'device', documentId: reference.documentId, sourceId: reference.sourceId, revision: reference.revision }
    : { kind: reference.sourceId.startsWith('provider:') ? 'provider' : 'local', sourceId: reference.sourceId, path: reference.path, revision: reference.revision });
  if (!parsed) throw new Error('This document has no valid exact reading receipt. Browse its source again.');
  return parsed;
}
export function presentation(entry: Pick<ReaderEntry, 'reference' | 'title' | 'heading'>): CanonicalReaderReference {
  const ref = entry.reference;
  return ref.kind === 'device'
    ? { documentId: ref.documentId, sourceId: ref.sourceId, path: 'document.md', revision: ref.revision, title: entry.title, line: 1 }
    : { sourceId: ref.sourceId, path: ref.path, revision: ref.revision, title: entry.title, line: 1, ...(entry.heading ? { heading: entry.heading } : {}) };
}
