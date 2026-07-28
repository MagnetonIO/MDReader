import type {
  EntityRow,
  LibraryEntityQueryResponse_Serialize,
  LibrarySnapshot,
  LibrarySourceExecution,
} from '../bindings.js';
import type { SettingsBackend } from '../platform/settings.js';

export const LIBRARY_DOCUMENT_FIELDS = [
  'providerId',
  'title',
  'author',
  'path',
  'mediaType',
  'modifiedAt',
  'size',
  'revision',
  'checksum',
  'body',
] as const;

const LOCAL_DOCUMENT_FIELDS = [
  'title',
  'author',
  'path',
  'modifiedAt',
  'size',
  'body',
] as const;

const DRIVE_DOCUMENT_FIELDS = [
  'title',
  'path',
  'mediaType',
  'modifiedAt',
  'size',
  'checksum',
  'body',
] as const;

const DROPBOX_DOCUMENT_FIELDS = [
  'providerId',
  'title',
  'path',
  'modifiedAt',
  'size',
  'revision',
  'checksum',
  'body',
] as const;

export interface HydratedLibrary {
  evidence: LibraryEntityQueryResponse_Serialize | null;
}

/** Canonical fields that at least one enabled Library membership can answer. */
export function queryableLibraryFields(snapshot: LibrarySnapshot): string[] {
  const queryable = new Set<string>();
  if (snapshot.sources.length > 0) {
    LOCAL_DOCUMENT_FIELDS.forEach((field) => queryable.add(field));
  }
  for (const source of snapshot.cloud_sources) {
    if (!source.enabled) continue;
    const fields =
      source.provider === 'google-drive'
        ? DRIVE_DOCUMENT_FIELDS
        : DROPBOX_DOCUMENT_FIELDS;
    fields.forEach((field) => queryable.add(field));
  }
  return LIBRARY_DOCUMENT_FIELDS.filter((field) => queryable.has(field));
}

/**
 * Resolve one Library snapshot and hydrate it through exactly one CatDB
 * entity-query call. Provider-specific query methods are intentionally absent
 * from this contract, so callers cannot concatenate independent answers.
 */
export async function hydrateLibrary(
  backend: Pick<
    SettingsBackend,
    'listSources' | 'prepareCloudCatalog' | 'queryLibraryDocuments'
  >,
  requestId: string,
): Promise<HydratedLibrary> {
  if (!backend.prepareCloudCatalog || !backend.queryLibraryDocuments) {
    throw new Error('Library queries are unavailable.');
  }
  const snapshot = await backend.listSources();
  const fields = queryableLibraryFields(snapshot);
  if (fields.length === 0) {
    return { evidence: null };
  }
  const catalog = await backend.prepareCloudCatalog();
  const evidence = await backend.queryLibraryDocuments(
    catalog.workspace_id,
    [],
    fields,
    requestId,
  );
  return { evidence };
}

export interface HydratedLibrarySelection {
  member: LibrarySourceExecution;
  row: EntityRow;
  evidence: LibraryEntityQueryResponse_Serialize;
}

function selectedSourceVersion(row: EntityRow): string | null {
  const path = row.provenance.path;
  if (!path || path.selected === null) return null;
  return path.considered[path.selected]?.source_version_id ?? null;
}

/** The provider-owned key selected for the semantic path identity. */
export function physicalRecordKey(row: EntityRow): string | null {
  const path = row.provenance.path;
  if (!path || path.selected === null) return null;
  return path.considered[path.selected]?.record_key ?? null;
}

/** Resolve a row to the membership that supplied its semantic path identity. */
export function memberForRow(
  evidence: LibraryEntityQueryResponse_Serialize,
  row: EntityRow,
): LibrarySourceExecution | null {
  const sourceVersionId = selectedSourceVersion(row);
  if (sourceVersionId === null) return null;
  const sourceId = evidence.answer.sources.find(
    (source) => source.source_version_id === sourceVersionId,
  )?.source_id;
  if (!sourceId) return null;
  return (
    evidence.memberships.find((membership) => membership.source_ids.includes(sourceId)) ?? null
  );
}

/** Rows whose identity was supplied by this exact Library membership. */
export function rowsForMember(
  evidence: LibraryEntityQueryResponse_Serialize,
  member: LibrarySourceExecution,
): EntityRow[] {
  return evidence.answer.rows.filter(
    (row) => memberForRow(evidence, row)?.membership_id === member.membership_id,
  );
}
