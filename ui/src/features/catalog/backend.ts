import type { LocalLinkResolution, LocalImageBytes } from '../reader/contracts';
import { readCatalogImage } from './local-image-backend';
import { readCatalogAttachment } from './attachment-backend';
import type { CanonicalReaderReference } from '../reader/contracts';
import type { ProviderReaderReference, ProviderDocumentBody } from '../../platform/providers';
import { commands, type CatalogRequest, type CatalogReply, type CatalogOpen, type AssetPreview } from '../../bindings';
export type { CatalogSource, CatalogDocument, CatalogOpen } from '../../bindings';
export type CatalogPage = Extract<CatalogReply, { operation: 'page' }>;
type CatalogList = Extract<CatalogReply, { operation: 'list' }>;

export interface CatalogBackend {
  list(): Promise<CatalogList>;
  page(catalogId: string, cursor: string | null, limit: number): Promise<CatalogPage>;
  close(cursor: string): Promise<void>;
  resolveLink?(
    reference: ProviderReaderReference, kind: 'relative' | 'wikilink', target: string,
  ): Promise<LocalLinkResolution>;
  readDocument(reference: ProviderReaderReference): Promise<ProviderDocumentBody>;
  readCurrentLocal?(sourceId: string, path: string): Promise<ProviderDocumentBody>;
  readImage?(reference: ProviderReaderReference, target: string): Promise<LocalImageBytes>;
  readAttachment?(reference: ProviderReaderReference, target: string): Promise<AssetPreview>;
}

export function catalogErrorMessage(kind: string): string {
  if (kind === 'stale') return 'The catalog changed. Reload this source before continuing.';
  if (kind === 'unauthorized') return 'This source is disabled, removed, or its retained folder identity changed.';
  if (kind === 'busy') return 'Library is temporarily busy. Refresh and try again.';
  if (kind === 'unsupported') return 'This source cannot open that kind of document or link. Open it in its original app.';
  if (kind === 'too_large') return 'This catalog request exceeds its supported bound.';
  return 'The catalog is unavailable. Existing grants and originals have not been changed.';
}

export class CatalogError extends Error {
  constructor(readonly kind: string) {
    super(catalogErrorMessage(kind));
  }
}

async function request<O extends CatalogReply['operation']>(
  value: CatalogRequest,
  operation: O,
): Promise<Extract<CatalogReply, { operation: O }>> {
  try {
    const response = await commands.readerCatalog(value);
    if (response.status === 'error') throw new CatalogError(response.error.kind);
    if (response.data.operation !== operation) throw new CatalogError('unavailable');
    return response.data as Extract<CatalogReply, { operation: O }>;
  } catch (cause) {
    throw cause instanceof CatalogError ? cause : new CatalogError('unavailable');
  }
}

export const catalogBackend: CatalogBackend = {
  readImage: readCatalogImage,
  readAttachment: readCatalogAttachment,
  list: () => request({ operation: 'list' }, 'list'),
  page: (catalogId, cursor, limit) =>
    request({ operation: 'page', catalogId, cursor, limit }, 'page'),
  async close(cursor) {
    await request({ operation: 'close', cursor }, 'complete');
  },
  async resolveLink(reference, kind, target) {
    const rawHeading = target.includes('#') ? target.slice(target.indexOf('#') + 1) : null;
    let heading: string | null;
    try { heading = rawHeading ? decodeURIComponent(rawHeading) : null; }
    catch { throw new CatalogError('invalid'); }
    const response = await request({
      operation: 'resolve', sourceId: reference.sourceId, path: reference.path,
      revision: reference.revision, kind, target,
    }, 'resolved');
    return { state: 'local_matches', linkKind: kind, references: response.references.map((value) => ({
      ...catalogReference({ kind: 'exact', ...value }, value.path), line: value.line, ...(heading ? { heading } : {}),
    })) };
  },
  async readCurrentLocal(sourceId, path) {
    const value = await request({ operation: 'read_current_local', sourceId, path }, 'read_document');
    if (value.sourceId !== sourceId || value.path !== path || value.sourceKind !== 'local') throw new CatalogError('invalid');
    catalogReference({ kind: 'exact', sourceId: value.sourceId, path: value.path, revision: value.revision }, path);
    return { operation: 'read_document', markdown: value.markdown, sourceId: value.sourceId,
      path: value.path, revision: value.revision, lineStart: value.lineStart, lineEnd: value.lineEnd, sourceKind: 'local' };
  },
  async readDocument(reference) {
    const value = await request({ operation: 'read', sourceId: reference.sourceId,
      path: reference.path, revision: reference.revision }, 'read_document');
    const kind = 'sourceKind' in value ? value.sourceKind : undefined;
    if (kind !== undefined && kind !== 'local' && kind !== 'provider' && kind !== 'capture') {
      throw new CatalogError('unsupported');
    }
    return { operation: 'read_document', markdown: value.markdown, sourceId: value.sourceId,
      path: value.path, revision: value.revision, lineStart: value.lineStart, lineEnd: value.lineEnd,
      sourceKind: kind };
  },
};

/** Never reinterpret a canonical receipt as an absolute local filesystem path. */
export function catalogReference(open: CatalogOpen, title: string): CanonicalReaderReference {
  if (open.kind === 'device') {
    if (!/^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/.test(open.documentId)
      || !/^[a-f0-9]{64}$/.test(open.sourceId) || !/^[a-f0-9]{64}$/.test(open.revision)
      || !Number.isSafeInteger(open.line) || open.line < 1) throw new CatalogError('invalid');
    return { documentId: open.documentId, sourceId: open.sourceId, revision: open.revision,
      path: 'document.md', title, line: open.line };
  }
  if (open.kind !== 'exact' || !/^[a-f0-9]{64}$/.test(open.revision)
    || !/^(?:[a-f0-9]{64}|provider:[a-f0-9]{64})$/.test(open.sourceId)
    || !open.path || open.path.length > 4096
    || open.path.includes('\\') || open.path.startsWith('/')
    || open.path.split('/').some((part) => !part || part === '.' || part === '..')
    || [...open.path].some((c) => c.charCodeAt(0) < 32 || c.charCodeAt(0) === 127)
    || open.sourceId.startsWith('provider:') && !/^records\/[a-f0-9]{64}\.md$/.test(open.path)) {
    throw new CatalogError('invalid');
  }
  return { sourceId: open.sourceId, path: open.path, revision: open.revision, title, line: 1 };
}
