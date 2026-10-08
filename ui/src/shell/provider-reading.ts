import type { HitTarget } from '@mdreader/ui';
import type { ProviderReaderReference, ProviderProjection } from '../platform/providers';

export const isCanonicalProviderSource = (value: string): boolean => /^provider:[a-f0-9]{64}$/.test(value);
/** A provider-looking target is never eligible for local filesystem fallback. */
export function providerTarget(target: HitTarget): ProviderReaderReference | null {
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
