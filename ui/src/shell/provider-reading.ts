import type { HitTarget } from '@mdreader/ui';
import type { CanonicalReaderReference } from '../features/reader/contracts';
import { receipt } from '../features/reader-history/references';
import type { ProviderReaderReference, ProviderProjection } from '../platform/providers';

export const isCanonicalProviderSource = (value: string): boolean => /^provider:[a-f0-9]{64}$/.test(value);
/** Service-owned search receipts never fall back to filesystem path opening. */
export function providerTarget(target: HitTarget): CanonicalReaderReference | null {
  if (target.deviceDocumentId != null || target.sourceKind === 'device') {
    if (!target.deviceDocumentId || !target.sourceId || !target.revision || target.path !== 'document.md')
      throw new Error('This device result has no valid exact revision. Search again.');
    const reference = { documentId: target.deviceDocumentId, sourceId: target.sourceId,
      path: target.path, revision: target.revision, title: target.name, line: target.line ?? 1 };
    receipt(reference);
    if (!Number.isSafeInteger(reference.line) || reference.line < 1) throw new Error('The reading position is invalid.');
    return reference;
  }
  if (target.sourceKind === 'capture' || target.sourceId && !target.sourceId.startsWith('provider:') && !/^(?:\/|[a-z]:[\\/])/i.test(target.path)) {
    if (!target.sourceId || !target.revision) throw new Error('This source result has no valid exact revision. Search again.');
    const reference = { sourceId: target.sourceId, path: target.path, revision: target.revision,
      title: target.name, line: target.line ?? 1 };
    receipt(reference);
    if (!Number.isSafeInteger(reference.line) || reference.line < 1) throw new Error('The reading position is invalid.');
    return reference;
  }
  if (!target.sourceId?.startsWith('provider:')) return null;
  if (!isCanonicalProviderSource(target.sourceId) || !/^records\/[a-f0-9]{64}\.md$/.test(target.path)
    || !target.revision || !/^[a-f0-9]{64}$/.test(target.revision)
    || target.line !== undefined && (!Number.isSafeInteger(target.line) || target.line < 1)) {
    throw new Error('This cloud result has no valid exact revision. Search again.');
  }
  return { sourceId: target.sourceId, path: target.path, revision: target.revision, title: target.name, line: target.line ?? 1 };
}
export function providerProjectionReference(projection: ProviderProjection, title: string): ProviderReaderReference {
  const reference = providerTarget({ sourceId: projection.readerSourceId, path: projection.path, revision: projection.revision, name: title, line: 1 });
  if (!reference) throw new Error('The cloud document reference is invalid.');
  return reference;
}
