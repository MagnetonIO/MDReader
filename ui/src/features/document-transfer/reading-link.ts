import type { CanonicalReaderReference } from '../reader/contracts';
import { presentation, receipt } from '../reader-history/references';
import { parseReference } from '../reader-history/serialization';

/** A locator within an already-authorized library, never a source grant. */
export function readingLink(reference: CanonicalReaderReference, line = reference.line ?? 1): string {
  if (!Number.isSafeInteger(line) || line < 1 || line > 10_000_000) throw new Error('The reading position is invalid.');
  const link = new URL('mdreader://read');
  link.searchParams.set('selection', JSON.stringify({ version: 1, receipt: receipt(reference), line }));
  if (link.href.length > 16384) throw new Error('This reading link exceeds the supported size.');
  return link.href;
}
export function parseReadingLink(raw: string): CanonicalReaderReference {
  if (raw.length > 16384) throw new Error('This reading link exceeds the supported size.');
  try {
    const url = new URL(raw);
    if (url.protocol !== 'mdreader:' || url.host !== 'read' || url.pathname || url.hash
      || url.username || url.password || [...url.searchParams.keys()].join(',') !== 'selection') throw new Error();
    const value = JSON.parse(url.searchParams.get('selection') ?? '') as Record<string, unknown>;
    const ref = parseReference(value.receipt);
    if (value.version !== 1 || Object.keys(value).sort().join(',') !== 'line,receipt,version'
      || !ref || typeof value.line !== 'number' || !Number.isSafeInteger(value.line) || value.line < 1 || value.line > 10_000_000) throw new Error();
    return { ...presentation({ reference: ref, title: '', heading: null }), line: value.line };
  } catch { throw new Error('Paste a supported MDReader reading link. No source will be authorized by this link.'); }
}
