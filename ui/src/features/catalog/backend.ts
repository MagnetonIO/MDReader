import type { LocalLinkResolution } from '../reader/contracts';
import type { CanonicalReaderReference } from '../reader/contracts';
import type { ProviderReaderReference, ProviderDocumentBody } from '../../platform/providers';
import { commands, type CatalogRequest, type CatalogReply, type CatalogOpen } from '../../bindings';
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
}

export function catalogErrorMessage(kind: string): string {
  if (kind === 'stale') return 'The catalog changed. Reload this source before continuing.';
  if (kind === 'unauthorized') return 'This source is disabled, removed, or its retained folder identity changed.';
  if (kind === 'busy') return 'Library is temporarily busy. Refresh and try again.';
  if (kind === 'unsupported') return 'This catalog namespace requires a supported reader version.';
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
  list: () => request({ operation: 'list' }, 'list'),
  page: (catalogId, cursor, limit) =>
    request({ operation: 'page', catalogId, cursor, limit }, 'page'),
  async close(cursor) {
    await request({ operation: 'close', cursor }, 'complete');
  },
  async resolveLink(reference, _kind, target) {
    const response = await request({
      operation: 'resolve', sourceId: reference.sourceId, path: reference.path,
      revision: reference.revision, target,
    }, 'resolved');
    return { state: 'local_matches', references: response.references.map((value) => ({
      ...catalogReference({ kind: 'exact', ...value }, value.path), line: value.line,
    })) };
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
