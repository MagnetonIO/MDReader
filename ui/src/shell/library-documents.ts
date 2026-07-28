import type {
  EntityRow,
  LibraryEntityQueryResponse_Serialize,
  LibrarySnapshot,
  LibrarySourceExecution,
} from '../bindings.js';
import { SettingsCommandError, type SettingsBackend } from '../platform/settings.js';

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
  /** Independent immutable catalog answers presented as one Library surface. */
  evidence: LibraryEntityQueryResponse_Serialize[];
  /** Catalog authorities that failed without suppressing healthy answers. */
  failures: LibraryHydrationFailure[];
}

export interface LibraryHydrationFailure {
  authority: 'embedded' | 'connected-catdb';
  /** Preserve the native typed error so the UI can offer the right recovery. */
  reason: unknown;
}

/** A reader-facing name from semantic fields, never CatDB's composite key when
 * the source supplied a usable path. A partial frontmatter execution may leave
 * `title` absent while filesystem still supplies `path`. */
export function libraryDocumentTitle(row: EntityRow): string {
  const fields =
    row.fields && typeof row.fields === 'object' && !Array.isArray(row.fields)
      ? (row.fields as Record<string, unknown>)
      : {};
  if (typeof fields.title === 'string' && fields.title.trim().length > 0) {
    return fields.title;
  }
  if (typeof fields.path === 'string' && fields.path.trim().length > 0) {
    return fields.path.replace(/[\\/]+$/, '').split(/[\\/]/).pop() ?? fields.path;
  }
  return row.record_key;
}

function localQueryableLibraryFields(snapshot: LibrarySnapshot): string[] {
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

/** Canonical fields that at least one enabled Library membership can answer. */
export function queryableLibraryFields(snapshot: LibrarySnapshot): string[] {
  if (snapshot.connected_catdb && snapshot.connected_catdb.selected_source_ids.length > 0) {
    return [...LIBRARY_DOCUMENT_FIELDS];
  }
  return localQueryableLibraryFields(snapshot);
}

/**
 * Resolve one Library snapshot and hydrate each catalog authority. Local and
 * cloud memberships share one embedded answer. A Connected CatDB stays a
 * separate evidence group because its remote catalog pins cannot be merged
 * honestly with the embedded catalog's pins.
 */
export async function hydrateLibrary(
  backend: Pick<
    SettingsBackend,
    | 'listSources'
    | 'prepareCloudCatalog'
    | 'queryLibraryDocuments'
    | 'queryConnectedCatdbDocuments'
  >,
  requestId: string,
): Promise<HydratedLibrary> {
  const snapshot = await backend.listSources();
  const reads: Array<{
    authority: LibraryHydrationFailure['authority'];
    execute: () => Promise<LibraryEntityQueryResponse_Serialize>;
  }> = [];
  const embeddedFields = localQueryableLibraryFields(snapshot);
  if (embeddedFields.length > 0) {
    reads.push({
      authority: 'embedded',
      execute: async () => {
        if (!backend.prepareCloudCatalog || !backend.queryLibraryDocuments) {
          throw new Error('Embedded Library queries are unavailable.');
        }
        const catalog = await backend.prepareCloudCatalog();
        return backend.queryLibraryDocuments(
          catalog.workspace_id,
          [],
          embeddedFields,
          requestId,
        );
      },
    });
  }
  if (snapshot.connected_catdb && snapshot.connected_catdb.selected_source_ids.length > 0) {
    const connectionId = snapshot.connected_catdb.connection_id;
    reads.push({
      authority: 'connected-catdb',
      execute: async () => {
        if (!backend.queryConnectedCatdbDocuments) {
          throw new Error('Connected CatDB Library queries are unavailable.');
        }
        return backend.queryConnectedCatdbDocuments(
          connectionId,
          [],
          [...LIBRARY_DOCUMENT_FIELDS],
          `${requestId}:connected-catdb`,
        );
      },
    });
  }
  const settled = await Promise.allSettled(reads.map(({ execute }) => execute()));
  return {
    evidence: settled.flatMap((result) => (result.status === 'fulfilled' ? [result.value] : [])),
    failures: settled.flatMap((result, index) =>
      result.status === 'rejected'
        ? [{ authority: reads[index]!.authority, reason: result.reason }]
        : [],
    ),
  };
}

function connectedCatdbFailureKind(failure: LibraryHydrationFailure): string | null {
  if (
    failure.authority !== 'connected-catdb' ||
    !(failure.reason instanceof SettingsCommandError)
  ) {
    return null;
  }
  const detail = failure.reason.detail;
  if (typeof detail !== 'object' || detail === null || !('kind' in detail)) return null;
  return typeof detail.kind === 'string' ? detail.kind : null;
}

function isSafeLocalMetadataPartial(
  answer: LibraryEntityQueryResponse_Serialize,
): boolean {
  const completeness = answer.answer.completeness;
  if (
    completeness.state !== 'partial' ||
    completeness.excluded_source_ids.length === 0
  ) {
    return false;
  }
  const localSourceIds = new Set(
    answer.memberships
      .filter((membership) => membership.kind === 'local-folder')
      .flatMap((membership) => membership.source_ids),
  );
  const hasUsableLocalRow = answer.answer.rows.some(
    (row) => memberForRow(answer, row)?.kind === 'local-folder',
  );
  return (
    hasUsableLocalRow &&
    completeness.excluded_source_ids.every((sourceId) => localSourceIds.has(sourceId))
  );
}

/** Reader-facing recovery for whole-authority failures and partial answers. */
export function libraryHydrationMessage(result: HydratedLibrary): string | null {
  const kinds = new Set(result.failures.map(connectedCatdbFailureKind));
  if (kinds.has('connection-identity-changed')) {
    return 'Connected CatDB changed identity. Reconnect it in Settings.';
  }
  if (kinds.has('permission-denied')) {
    return 'Connected CatDB denied access. Reconnect it in Settings.';
  }
  if (
    result.failures.length === 0 &&
    result.evidence.every((answer) => answer.answer.completeness.state !== 'partial')
  ) {
    return null;
  }
  const partialAnswers = result.evidence.filter(
    (answer) => answer.answer.completeness.state === 'partial',
  );
  if (
    result.failures.length === 0 &&
    partialAnswers.length > 0 &&
    partialAnswers.every(isSafeLocalMetadataPartial)
  ) {
    return null;
  }
  return result.evidence.length === 0
    ? 'Library sources could not be read.'
    : 'Some Library sources could not answer this query.';
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
