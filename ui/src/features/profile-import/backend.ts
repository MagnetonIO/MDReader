/** Native dialogs own all source/destination selection and offline authority. */
import { invoke } from '@tauri-apps/api/core';

export type ImportDocument = {
  documentId: string;
  name: string;
  revision: string;
  bytes: number;
  pinned: boolean;
  line: number;
};
export type ImportCoverage = {
  documents: number;
  sourceReferences: number;
  relationships: number;
  reviews: number;
  historyEntries: number;
  unavailableHistoryEntries: number;
  settings: boolean;
  unknownRecords: number;
};
export type ImportReply = {
  origin: 'reader' | 'legacy_catdb' | null;
  coverage: ImportCoverage | null;
  state: 'close_original_required' | 'background_service_unsupported' | 'service_handoff_pending' | 'rollback_handoff_pending' | 'preview' | 'exported' | 'prepared' | 'activated' | 'launch_requested' | 'rollback_launch_requested' | 'rolled_back' | 'cancelled' | 'closed';
  operationId: string | null;
  documents: ImportDocument[];
  imported: number;
};
export type ImportRequest =
  | { operation: 'preview' | 'resume' }
  | { operation: 'retry' | 'export' | 'activate' | 'rollback' | 'close'; operationId: string }
  | { operation: 'prepare'; operationId: string; documents: { documentId: string; revision: string }[] };

export function isWaitingImport(value: ImportReply): boolean {
  return ['close_original_required', 'background_service_unsupported', 'service_handoff_pending', 'rollback_handoff_pending'].includes(value.state);
}

const errors: Record<string, string> = {
  close_source_app_required: 'Close the app that is using the library you selected, then choose it again. This window and its drafts can stay open.',
  background_service_unsupported: 'The original background service does not support this safe import handoff. Your originals were kept, and the selected import remains pending.',
  service_handoff_unconfirmed: 'The original background service handoff could not be confirmed. Your originals and the pending selection were kept; no completed copy is being claimed.',
  offline_export_required: 'This library contains a storage format MDReader cannot import yet. Your originals were kept unchanged.',
  too_large: 'This library exceeds the supported import limits. Its originals were kept.',
  profile_changed: 'The selected library changed. Choose it again before importing; no newer version was substituted.',
  invalid_profile: 'That folder is not a supported reader library or prepared import. Choose the original library folder.',
  invalid_destination: 'Choose a different destination folder outside both the original library and the library open in this window. No copy was created.',
  migration_unconfirmed: 'The import could not be confirmed. Keep your originals and any copies you created. Use Continue an import to inspect a prepared copy before retrying.',
};

export class ImportFailure extends Error {
  constructor(readonly kind: string) { super(errors[kind] ?? errors.migration_unconfirmed); }
}

function checked(value: ImportReply): ImportReply {
  const uuid = /^[a-f0-9]{8}-[a-f0-9]{4}-4[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}$/;
  const legacyDocumentId = /^[a-f0-9]{8}-[a-f0-9]{4}-[45][a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}$/;
  const uint = (n: number, max: number) => Number.isInteger(n) && n >= 0 && n <= max;
  if (!value || !['close_original_required', 'background_service_unsupported', 'service_handoff_pending', 'rollback_handoff_pending', 'preview', 'exported', 'prepared', 'activated', 'launch_requested', 'rollback_launch_requested', 'rolled_back', 'cancelled', 'closed'].includes(value.state)
    || value.operationId !== null && !uuid.test(value.operationId)
    || !['reader', 'legacy_catdb', null].includes(value.origin)
    || !uint(value.imported, 1000) || !Array.isArray(value.documents) || value.documents.length > 1000) throw new ImportFailure('migration_unconfirmed');
  if (isWaitingImport(value) && (!['reader', 'legacy_catdb'].includes(value.origin ?? '') || value.coverage !== null
    || value.documents.length !== 0 || value.imported !== 0)) throw new ImportFailure('migration_unconfirmed');
  if (value.coverage !== null) {
    const coverage = value.coverage;
    if (!coverage || typeof coverage.settings !== 'boolean'
      || ![coverage.documents, coverage.sourceReferences, coverage.relationships, coverage.reviews, coverage.historyEntries,
        coverage.unavailableHistoryEntries, coverage.unknownRecords].every((count) => uint(count, 1_000_000))
      || coverage.unavailableHistoryEntries > coverage.historyEntries) throw new ImportFailure('migration_unconfirmed');
  }
  if (value.origin === 'legacy_catdb' && value.state === 'preview' && !value.coverage) throw new ImportFailure('migration_unconfirmed');
  const ids = new Set<string>();
  for (const document of value.documents) {
    if (!document || !(value.origin === 'legacy_catdb' ? legacyDocumentId : uuid).test(document.documentId) || ids.has(document.documentId)
      || typeof document.name !== 'string' || new TextEncoder().encode(document.name).length > 240
      || !/^[a-f0-9]{64}$/.test(document.revision) || !uint(document.bytes, 1024 * 1024)
      || !uint(document.line, 0xffff_ffff) || typeof document.pinned !== 'boolean') throw new ImportFailure('migration_unconfirmed');
    ids.add(document.documentId);
  }
  if (!['cancelled', 'closed'].includes(value.state) && !value.operationId) throw new ImportFailure('migration_unconfirmed');
  return value;
}

export async function importRequest(request: ImportRequest): Promise<ImportReply> {
  try {
    const value = checked(await invoke<ImportReply>('native_profile_migration', { request }));
    if ('operationId' in request && value.operationId !== request.operationId) throw new ImportFailure('migration_unconfirmed');
    return value;
  } catch (cause) {
    if (cause instanceof ImportFailure) throw cause;
    const kind = cause && typeof cause === 'object' && 'kind' in cause ? String(cause.kind) : '';
    throw new ImportFailure(kind);
  }
}
