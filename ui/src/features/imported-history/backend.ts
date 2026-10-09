import { invoke } from '@tauri-apps/api/core';
import type { CanonicalReaderReference } from '../reader/contracts';
import { parseReference } from '../reader-history/serialization';
import { presentation, receipt } from '../reader-history/references';

export interface ImportedHistoryEntry {
  entryId: string;
  title: string;
  kind: 'local' | 'provider' | 'legacy_record' | 'unknown';
  status: 'unresolved' | 'bound';
  context: 'tab' | 'tab_history' | 'recent' | 'source_document' | 'unknown';
  archivedPosition: boolean;
  reference: CanonicalReaderReference | null;
}
export interface ImportedHistoryPage {
  entries: ImportedHistoryEntry[];
  historyDigest: string;
  nextAfterEntryId: string | null;
}
export interface ImportedHistoryBackend {
  list(afterEntryId: string | null): Promise<ImportedHistoryPage>;
  bind(entry: ImportedHistoryEntry, historyDigest: string, reference: CanonicalReaderReference): Promise<ImportedHistoryPage>;
}
type NativeReference = { documentId: string | null; sourceId: string; path: string; revision: string };
type NativePage = Omit<ImportedHistoryPage, 'entries'> & { entries: (Omit<ImportedHistoryEntry, 'reference'> & { reference: NativeReference | null })[] };
const hash = /^[a-f0-9]{64}$/;
const identifier = (value: unknown): value is string => typeof value === 'string' && value.length > 0 && value.length <= 256 && !/[\u0000-\u001f\u007f]/.test(value);

export class ImportedHistoryFailure extends Error {
  constructor(readonly kind: string) {
    super(kind === 'stale' || kind === 'conflict' ? 'This history or selected copy changed. Refresh history before matching it again.'
      : kind === 'busy' ? 'The library is busy. Refresh history when the current operation finishes.'
      : kind === 'unsupported' ? 'This imported history needs a supported reader version. Its original records are kept.'
      : kind === 'too_large' ? 'This history exceeds the supported reading bounds. Its original records are kept.'
      : 'The historical match could not be confirmed. Its original record was kept. Refresh history before trying again.');
  }
}
function decode(value: NativePage): ImportedHistoryPage {
  if (!value || !hash.test(value.historyDigest) || !Array.isArray(value.entries) || value.entries.length > 100
    || value.nextAfterEntryId !== null && !identifier(value.nextAfterEntryId)) throw new ImportedHistoryFailure('unavailable');
  const seen = new Set<string>();
  const entries = value.entries.map((entry): ImportedHistoryEntry => {
    if (!entry || !identifier(entry.entryId) || seen.has(entry.entryId)
      || typeof entry.title !== 'string' || new TextEncoder().encode(entry.title).length > 512
      || !['local', 'provider', 'legacy_record', 'unknown'].includes(entry.kind)
      || !['tab', 'tab_history', 'recent', 'source_document', 'unknown'].includes(entry.context)
      || !['unresolved', 'bound'].includes(entry.status) || typeof entry.archivedPosition !== 'boolean'
      || (entry.status === 'bound') !== (entry.reference !== null)) throw new ImportedHistoryFailure('unavailable');
    seen.add(entry.entryId);
    let reference: CanonicalReaderReference | null = null;
    if (entry.reference) {
      const native = entry.reference;
      if (typeof native.sourceId !== 'string' || typeof native.path !== 'string' || typeof native.revision !== 'string'
        || native.documentId !== null && typeof native.documentId !== 'string') throw new ImportedHistoryFailure('unavailable');
      const parsed = parseReference(native.documentId !== null
        ? { kind: 'device', documentId: native.documentId, sourceId: native.sourceId, revision: native.revision }
        : { kind: native.sourceId.startsWith('provider:') ? 'provider' : 'local', ...native });
      if (!parsed || parsed.kind === 'device' && native.path !== 'document.md') throw new ImportedHistoryFailure('unavailable');
      reference = presentation({ reference: parsed, title: entry.title, heading: null });
    }
    return { ...entry, reference };
  });
  return { ...value, entries };
}
async function request(request: Record<string, unknown>): Promise<ImportedHistoryPage> {
  try { return decode(await invoke<NativePage>('native_profile_imported_history', { request })); }
  catch (cause) {
    if (cause instanceof ImportedHistoryFailure) throw cause;
    const kind = cause && typeof cause === 'object' && 'kind' in cause ? String(cause.kind) : 'unavailable';
    throw new ImportedHistoryFailure(kind);
  }
}
export const importedHistoryBackend: ImportedHistoryBackend = {
  list: (afterEntryId) => request({ operation: 'list', afterEntryId }),
  async bind(entry, historyDigest, reference) {
    receipt(reference); // History references are receipts, never new grants.
    const value = await request({ operation: 'bind', entryId: entry.entryId, historyDigest,
      reference: { documentId: reference.documentId ?? null, sourceId: reference.sourceId, path: reference.path, revision: reference.revision } });
    const bound = value.entries.find((item) => item.entryId === entry.entryId);
    if (bound?.status !== 'bound' || !bound.reference || bound.reference.documentId !== reference.documentId
      || bound.reference.sourceId !== reference.sourceId || bound.reference.path !== reference.path
      || bound.reference.revision !== reference.revision) throw new ImportedHistoryFailure('unavailable');
    return value;
  },
};
