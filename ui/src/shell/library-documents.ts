import type {
  CloudMarkdownDocument,
  EntityRow,
  LibraryEntityQueryResponse_Serialize,
  LibrarySnapshot,
  LibrarySourceExecution,
  SourceExecution,
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
] as const;

const LIBRARY_DOCUMENT_BODY_FIELDS = ['body'] as const;

const LOCAL_DOCUMENT_FIELDS = [
  'title',
  'author',
  'path',
  'modifiedAt',
  'size',
] as const;

const DRIVE_DOCUMENT_FIELDS = [
  'title',
  'path',
  'mediaType',
  'modifiedAt',
  'size',
  'checksum',
] as const;

const DROPBOX_DOCUMENT_FIELDS = [
  'providerId',
  'title',
  'path',
  'modifiedAt',
  'size',
  'revision',
  'checksum',
] as const;

export interface HydratedLibrary {
  /** Independent immutable catalog answers presented as one Library surface. */
  evidence: LibraryEntityQueryResponse_Serialize[];
  /** Catalog authorities that failed without suppressing healthy answers. */
  failures: LibraryHydrationFailure[];
  /** Native application continuation for the embedded answer, when available. */
  continuation?: HydratedLibraryContinuation;
}

export interface HydratedLibraryContinuation {
  workspaceId: string;
  fields: string[];
  queryId: string;
  /** Opaque application cursor; provider continuations never enter the UI. */
  cursor: string;
}

export interface LibraryHydrationFailure {
  authority: 'embedded' | 'connected-catdb';
  /** Preserve the native typed error so the UI can offer the right recovery. */
  reason: unknown;
}

/** The reader-facing freshness state of the published Library materialization. */
export type LibraryAvailability =
  | { kind: 'ready' }
  | { kind: 'syncing'; sourceNames: string[] }
  | { kind: 'stale'; sourceNames: string[] }
  | { kind: 'partial'; sourceNames: string[] }
  | { kind: 'failed'; message: string };

function publishedSourceNames(result: HydratedLibrary): string[] {
  return [
    ...new Set(
      result.evidence.flatMap((answer) =>
        answer.memberships.map((membership) => membership.name),
      ),
    ),
  ];
}

function sourceNamesFor(
  result: HydratedLibrary,
  sourceIds: ReadonlySet<string>,
): string[] {
  const names = [
    ...new Set(
      result.evidence.flatMap((answer) =>
        answer.memberships
          .filter(
            (membership) =>
              sourceIds.size === 0 ||
              membership.source_ids.some((sourceId) => sourceIds.has(sourceId)),
          )
          .map((membership) => membership.name),
      ),
    ),
  ];
  return names.length > 0 ? names : publishedSourceNames(result);
}

function sourceIdsFromFailure(failure: LibraryHydrationFailure): string[] {
  if (!(failure.reason instanceof SettingsCommandError)) return [];
  const detail = failure.reason.detail;
  if (typeof detail !== 'object' || detail === null) return [];
  const record = detail as Record<string, unknown>;
  if (typeof record.source_id === 'string') return [record.source_id];
  if (Array.isArray(record.source_ids)) {
    return record.source_ids.filter((sourceId): sourceId is string => typeof sourceId === 'string');
  }
  return [];
}

function failureHasKind(
  failure: LibraryHydrationFailure,
  kind: string,
): boolean {
  return commandFailureKind(failure) === kind;
}

/**
 * Classify published query evidence without treating a background refresh
 * failure as a failure to read rows that were already materialized.
 */
export function libraryAvailability(result: HydratedLibrary): LibraryAvailability {
  const hasPublishedRows = result.evidence.some((answer) => answer.answer.rows.length > 0);
  const staleSourceIds = new Set(
    result.evidence.flatMap((answer) =>
      answer.answer.sources
        .filter((source) => source.outcome === 'stale')
        .map((source) => source.source_id),
    ),
  );
  const partialSourceIds = new Set(
    result.evidence.flatMap((answer) =>
      answer.answer.completeness.state === 'partial'
        ? answer.answer.completeness.excluded_source_ids
        : [],
    ),
  );
  const failedSourceIds = new Set(result.failures.flatMap(sourceIdsFromFailure));
  if (
    hasPublishedRows &&
    (staleSourceIds.size > 0 ||
      result.failures.some((failure) => failureHasKind(failure, 'query-stale-sources')))
  ) {
    return {
      kind: 'stale',
      sourceNames: sourceNamesFor(
        result,
        new Set([...staleSourceIds, ...failedSourceIds]),
      ),
    };
  }
  if (hasPublishedRows && result.failures.length > 0) {
    return {
      kind: 'partial',
      sourceNames: sourceNamesFor(result, failedSourceIds),
    };
  }
  const partialAnswers = result.evidence.filter(
    (answer) => answer.answer.completeness.state === 'partial',
  );
  if (
    partialAnswers.length > 0 &&
    !partialAnswers.every(isSafeLocalMetadataPartial)
  ) {
    return {
      kind: 'partial',
      sourceNames: sourceNamesFor(result, partialSourceIds),
    };
  }
  if (result.failures.length > 0) {
    return {
      kind: 'failed',
      message: libraryHydrationMessage(result) ?? 'Library sources could not be read.',
    };
  }
  return { kind: 'ready' };
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
  let embeddedContext: Pick<
    HydratedLibraryContinuation,
    'workspaceId' | 'fields'
  > | null = null;
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
        embeddedContext = {
          workspaceId: catalog.workspace_id,
          fields: [...embeddedFields],
        };
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
  const embeddedIndex = reads.findIndex(({ authority }) => authority === 'embedded');
  const embedded =
    embeddedIndex >= 0 ? settled[embeddedIndex] : undefined;
  const continuation =
    embedded?.status === 'fulfilled' && embeddedContext !== null
      ? nextLibraryContinuation(embedded.value, embeddedContext)
      : null;
  return {
    evidence: settled.flatMap((result) => (result.status === 'fulfilled' ? [result.value] : [])),
    failures: settled.flatMap((result, index) =>
      result.status === 'rejected'
        ? [{ authority: reads[index]!.authority, reason: result.reason }]
        : [],
    ),
    ...(continuation === null ? {} : { continuation }),
  };
}

function nextLibraryContinuation(
  response: LibraryEntityQueryResponse_Serialize,
  context: Pick<HydratedLibraryContinuation, 'workspaceId' | 'fields'>,
): HydratedLibraryContinuation | null {
  const cursor = response.answer.next_cursor;
  if (!response.window_truncated || cursor === null) return null;
  return {
    ...context,
    queryId: response.answer.query_id,
    cursor,
  };
}

function mergeSourceEvidence(
  previous: SourceExecution[],
  next: SourceExecution[],
): SourceExecution[] {
  const merged = new Map(previous.map((source) => [source.source_id, source]));
  for (const source of next) {
    const earlier = merged.get(source.source_id);
    if (!earlier || source.outcome !== 'ran' || earlier.outcome === 'ran') {
      merged.set(source.source_id, source);
    }
  }
  return [...merged.values()];
}

function mergeLibraryPages(
  previous: LibraryEntityQueryResponse_Serialize,
  next: LibraryEntityQueryResponse_Serialize,
): LibraryEntityQueryResponse_Serialize {
  if (
    previous.answer.query_id !== next.answer.query_id ||
    JSON.stringify(previous.answer.pins) !== JSON.stringify(next.answer.pins)
  ) {
    throw new Error('Library continuation changed immutable query authority.');
  }
  const rows = new Map(previous.answer.rows.map((row) => [row.record_key, row]));
  next.answer.rows.forEach((row) => rows.set(row.record_key, row));
  const memberships = new Map(
    previous.memberships.map((membership) => [
      membership.membership_id,
      membership,
    ]),
  );
  next.memberships.forEach((membership) =>
    memberships.set(membership.membership_id, membership),
  );
  const excluded = new Set<string>();
  for (const completeness of [
    previous.answer.completeness,
    next.answer.completeness,
  ]) {
    if (completeness.state === 'partial') {
      completeness.excluded_source_ids.forEach((sourceId) =>
        excluded.add(sourceId),
      );
    }
  }
  return {
    answer: {
      ...next.answer,
      rows: [...rows.values()],
      sources: mergeSourceEvidence(
        previous.answer.sources,
        next.answer.sources,
      ),
      completeness:
        excluded.size === 0
          ? { state: 'complete' }
          : {
              state: 'partial',
              excluded_source_ids: [...excluded].sort(),
            },
    },
    memberships: [...memberships.values()],
    window_truncated: next.window_truncated,
  };
}

/**
 * Advance exactly one native application page and retain prior rows and source
 * evidence. Provider cursors remain inside the native continuation registry.
 */
export async function continueHydratedLibrary(
  backend: Pick<SettingsBackend, 'queryLibraryDocuments'>,
  current: HydratedLibrary,
  requestId: string,
): Promise<HydratedLibrary> {
  const continuation = current.continuation;
  if (!continuation || !backend.queryLibraryDocuments) {
    throw new Error('Library continuation is unavailable.');
  }
  const evidenceIndex = current.evidence.findIndex(
    (answer) => answer.answer.query_id === continuation.queryId,
  );
  if (evidenceIndex < 0) {
    throw new Error('Library continuation has no matching evidence.');
  }
  const page = await backend.queryLibraryDocuments(
    continuation.workspaceId,
    [],
    continuation.fields,
    requestId,
    undefined,
    {
      queryId: continuation.queryId,
      cursor: continuation.cursor,
    },
  );
  const merged = mergeLibraryPages(current.evidence[evidenceIndex]!, page);
  const evidence = [...current.evidence];
  evidence[evidenceIndex] = merged;
  const next = nextLibraryContinuation(page, continuation);
  return {
    evidence,
    failures: current.failures,
    ...(next === null ? {} : { continuation: next }),
  };
}

function commandFailureKind(failure: LibraryHydrationFailure): string | null {
  if (!(failure.reason instanceof SettingsCommandError)) return null;
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
  const activeLocalSourceIds = new Set(
    answer.memberships
      .filter((membership) => membership.kind === 'local-folder' && membership.active)
      .flatMap((membership) => membership.source_ids),
  );
  const healthySourceIds = new Set(
    answer.answer.sources
      .filter((source) => source.outcome === 'ran')
      .map((source) => source.source_id),
  );
  return (
    [...activeLocalSourceIds].some((sourceId) => healthySourceIds.has(sourceId)) &&
    completeness.excluded_source_ids.every((sourceId) =>
      activeLocalSourceIds.has(sourceId),
    )
  );
}

/** Reader-facing recovery for whole-authority failures and partial answers. */
export function libraryHydrationMessage(result: HydratedLibrary): string | null {
  const connectedKinds = new Set(
    result.failures
      .filter((failure) => failure.authority === 'connected-catdb')
      .map(commandFailureKind),
  );
  if (connectedKinds.has('connection-identity-changed')) {
    return 'Connected CatDB changed identity. Reconnect it in Settings.';
  }
  if (connectedKinds.has('permission-denied')) {
    return 'Connected CatDB denied access. Reconnect it in Settings.';
  }
  const embeddedKinds = new Set(
    result.failures
      .filter((failure) => failure.authority === 'embedded')
      .map(commandFailureKind),
  );
  if (
    embeddedKinds.has('credential-missing') ||
    embeddedKinds.has('credential-unavailable') ||
    embeddedKinds.has('query-authentication')
  ) {
    return 'A connected Library source needs to be reconnected in Settings.';
  }
  if (
    embeddedKinds.has('query-deadline-exceeded') ||
    embeddedKinds.has('query-timeout')
  ) {
    return 'Library refresh timed out while reading a source. Try Refresh again.';
  }
  if (embeddedKinds.has('query-rate-limited')) {
    return 'A Library source is temporarily rate-limited. Try Refresh again shortly.';
  }
  if (embeddedKinds.has('query-limit-exceeded')) {
    return 'This Library is larger than the current refresh limit.';
  }
  if (
    embeddedKinds.has('query-catalog-invalid') ||
    embeddedKinds.has('query-catalog-unavailable') ||
    embeddedKinds.has('query-schema-drift')
  ) {
    return 'Library metadata is out of date. Refresh the Library or reconnect the source.';
  }
  if (
    embeddedKinds.has('query-read-failed') ||
    embeddedKinds.has('query-transport') ||
    embeddedKinds.has('query-source-protocol') ||
    embeddedKinds.has('query-execution-failed')
  ) {
    return 'A Library source failed while CatDB was reading it.';
  }
  if (
    embeddedKinds.has('query-invalid') ||
    embeddedKinds.has('query-invalid-response') ||
    embeddedKinds.has('query-failed')
  ) {
    return 'CatDB could not prepare or complete the Library query.';
  }
  if (
    result.evidence.some(
      (answer) => answer.window_truncated && answer.answer.next_cursor === null,
    )
  ) {
    return 'This Library is larger than the current refresh limit.';
  }
  if (result.evidence.some((answer) => answer.window_truncated)) {
    return 'More Library documents are available from this result.';
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
  /** Provider-owned body returned by the native point-read command. */
  cloudDocument?: CloudMarkdownDocument;
}

/**
 * Read the selected document body without hydrating every Library row. The
 * semantic `path` identity is mapped to Drive's file id, Dropbox's provider
 * path, or the local relative path, and the exact source membership is pinned
 * for the follow-up query.
 */
export async function hydrateLibraryDocument(
  backend: Pick<
    SettingsBackend,
    | 'prepareCloudCatalog'
    | 'queryLibraryDocuments'
    | 'queryConnectedCatdbDocuments'
    | 'readCloudMarkdown'
  >,
  selection: HydratedLibrarySelection,
  requestId: string,
): Promise<HydratedLibrarySelection> {
  const recordKey = physicalRecordKey(selection.row);
  if (recordKey === null) {
    throw new Error('The selected Library document has no provider identity.');
  }
  if (selection.member.kind === 'google-drive' || selection.member.kind === 'dropbox') {
    if (!backend.readCloudMarkdown) {
      throw new Error('Cloud document reads are unavailable.');
    }
    const cloudDocument = await backend.readCloudMarkdown(
      selection.member.registry_id,
      recordKey,
    );
    return { ...selection, cloudDocument };
  }
  const filters = [{ field: 'path', operator: 'eq' as const, value: recordKey }];
  let evidence: LibraryEntityQueryResponse_Serialize;
  if (selection.member.kind === 'connected-catdb') {
    if (!backend.queryConnectedCatdbDocuments) {
      throw new Error('Connected CatDB document reads are unavailable.');
    }
    evidence = await backend.queryConnectedCatdbDocuments(
      selection.member.registry_id,
      filters,
      [...LIBRARY_DOCUMENT_BODY_FIELDS],
      requestId,
    );
  } else {
    if (!backend.prepareCloudCatalog || !backend.queryLibraryDocuments) {
      throw new Error('Embedded Library document reads are unavailable.');
    }
    const catalog = await backend.prepareCloudCatalog();
    evidence = await backend.queryLibraryDocuments(
      catalog.workspace_id,
      filters,
      [...LIBRARY_DOCUMENT_BODY_FIELDS],
      requestId,
      [selection.member.membership_id],
    );
  }
  const member =
    evidence.memberships.find(
      (candidate) => candidate.membership_id === selection.member.membership_id,
    ) ?? null;
  const row =
    member === null
      ? null
      : rowsForMember(evidence, member).find(
          (candidate) => physicalRecordKey(candidate) === recordKey,
        ) ?? null;
  if (
    member === null ||
    row === null ||
    typeof (row.fields as Record<string, unknown> | null)?.body !== 'string'
  ) {
    throw new Error('The selected Library document body is unavailable.');
  }
  return { member, row, evidence };
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
