import type {
  CloudDocumentQueryResponse,
  CloudDocumentRow,
  CloudFilterInput,
  CloudProvider,
  CloudSourceDto,
} from '../bindings.js';
import { SettingsCommandError } from '../platform/settings.js';

export const CLOUD_DOCUMENT_FIELDS = [
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

export interface HydratedCloudSelection {
  source: CloudSourceDto;
  row: CloudDocumentRow;
  evidence: CloudDocumentQueryResponse;
}

interface CloudSelectionControllerOptions {
  query: (
    provider: CloudProvider,
    filters: CloudFilterInput[],
    fields: string[],
    requestId: string,
  ) => Promise<CloudDocumentQueryResponse>;
  cancel: (requestId: string) => Promise<boolean>;
  publish: (selection: HydratedCloudSelection) => void;
  fail: (message: string) => void;
}

function failureMessage(cause: unknown): string {
  if (
    cause instanceof SettingsCommandError &&
    typeof cause.detail === 'object' &&
    cause.detail !== null &&
    'kind' in cause.detail &&
    cause.detail.kind === 'query-not-found'
  ) {
    return 'The selected provider document was not found. Refresh the Library and try again.';
  }
  return 'The selected provider document could not be read. Refresh the Library and try again.';
}

/**
 * Owns the pre-publication read for a Sidebar cloud selection.
 *
 * A selection does not become reader state until its exact provider row has a
 * body. Replacements retire the previous request synchronously; late results
 * can therefore neither take reader ownership nor mount semantic inspection.
 */
export function createCloudSelectionController(
  options: CloudSelectionControllerOptions,
): {
  select: (source: CloudSourceDto, row: CloudDocumentRow) => Promise<void>;
  cancel: () => void;
} {
  let generation = 0;
  let activeRequestId: string | null = null;

  const cancel = () => {
    generation += 1;
    const requestId = activeRequestId;
    activeRequestId = null;
    if (requestId !== null) void options.cancel(requestId);
  };

  const select = async (source: CloudSourceDto, row: CloudDocumentRow) => {
    const selectionGeneration = generation + 1;
    generation = selectionGeneration;
    const retiredRequestId = activeRequestId;
    if (retiredRequestId !== null) void options.cancel(retiredRequestId);
    const requestId =
      `cloud-reader:${source.provider}:` + globalThis.crypto.randomUUID();
    activeRequestId = requestId;

    try {
      const evidence = await options.query(
        source.provider,
        [{ field: 'path', operator: 'eq', value: row.record_key }],
        [...CLOUD_DOCUMENT_FIELDS],
        requestId,
      );
      if (generation !== selectionGeneration) return;
      const hydrated = evidence.rows.find(
        (candidate) =>
          candidate.source_id === source.id &&
          candidate.record_key === row.record_key,
      );
      if (
        !hydrated ||
        !hydrated.fields ||
        typeof hydrated.fields !== 'object' ||
        Array.isArray(hydrated.fields) ||
        typeof (hydrated.fields as Record<string, unknown>).body !== 'string'
      ) {
        options.fail(
          'The selected provider document was not found. Refresh the Library and try again.',
        );
        return;
      }
      options.publish({ source, row: hydrated, evidence });
    } catch (cause: unknown) {
      if (generation === selectionGeneration) {
        options.fail(failureMessage(cause));
      }
    } finally {
      if (generation === selectionGeneration) activeRequestId = null;
    }
  };

  return { select, cancel };
}
