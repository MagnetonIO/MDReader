import type { CanonicalReaderReference } from '../reader/contracts';

/** A filename suggestion only. The native dialog still owns the destination. */
export function documentFilename(reference: Pick<CanonicalReaderReference, 'sourceId' | 'path' | 'title' | 'documentId'>): string {
  const selected = reference.documentId || reference.sourceId.startsWith('provider:')
    ? reference.title || reference.path : reference.path || reference.title;
  const leaf = selected.split(/[\\/]/).at(-1) || 'document';
  const name = leaf.replace(/[\u0000-\u001f\u007f<>:"|?*]/g, '_').replace(/[. ]+$/, '').slice(0, 180) || 'document';
  return /\.(md|markdown)$/i.test(name) ? name : `${name}.md`;
}
