import { expect, it, vi } from 'vitest';

import type {
  CloudDocumentQueryResponse,
  CloudDocumentRow,
  CloudSourceDto,
} from '../bindings.js';
import { SettingsCommandError } from '../platform/settings.js';
import { createCloudSelectionController } from './cloud-selection.js';

const source: CloudSourceDto = {
  id: 'source-1',
  provider: 'dropbox',
  label: 'Dropbox',
  connection: 'connected',
  enabled: true,
};

function answer(recordKey: string, body?: string): CloudDocumentQueryResponse {
  const row: CloudDocumentRow = {
    source_id: source.id,
    record_key: recordKey,
    fields: {
      path: recordKey,
      title: recordKey,
      ...(body === undefined ? {} : { body }),
    },
    provenance: [],
  };
  return {
    query_id: `query:${recordKey}`,
    provider: source.provider,
    workspace_id: '00000000-0000-4000-8000-000000000001',
    schema_version: '00000000-0000-4000-8000-000000000003',
    version_pins: {
      catalog_version: '00000000-0000-4000-8000-000000000002',
      schema_version: '00000000-0000-4000-8000-000000000003',
      schema_checksum: 'schema',
      source_schema_version: '00000000-0000-4000-8000-000000000004',
      source_schema_checksum: 'source-schema',
      mapping_version: '00000000-0000-4000-8000-000000000005',
      mapping_checksum: 'mapping',
      connector_version: 'dropbox/v1',
      source_version: '00000000-0000-4000-8000-000000000006',
      source_checksum: 'source',
      policy_version: '00000000-0000-4000-8000-000000000007',
      policy_checksum: 'policy',
    },
    completeness: 'complete',
    sources: [],
    scanned: 1,
    rows: [row],
  };
}

it('publishes cloud ownership only after the selected body hydrates', async () => {
  let release!: (response: CloudDocumentQueryResponse) => void;
  const pending = new Promise<CloudDocumentQueryResponse>((resolve) => {
    release = resolve;
  });
  const publish = vi.fn();
  const controller = createCloudSelectionController({
    query: vi.fn(async () => pending),
    cancel: vi.fn(async () => false),
    publish,
    fail: vi.fn(),
  });
  const selection = controller.select(source, answer('/note.md').rows[0]!);

  expect(publish).not.toHaveBeenCalled();
  release(answer('/note.md', '# Hydrated'));
  await selection;

  expect(publish).toHaveBeenCalledOnce();
  expect(publish.mock.calls[0]![0].row.fields).toMatchObject({ body: '# Hydrated' });
});

it('cancels and discards a stale hydration when a newer selection wins', async () => {
  let releaseFirst!: (response: CloudDocumentQueryResponse) => void;
  const first = new Promise<CloudDocumentQueryResponse>((resolve) => {
    releaseFirst = resolve;
  });
  const publish = vi.fn();
  const cancel = vi.fn(async () => true);
  const controller = createCloudSelectionController({
    query: vi.fn(async (_provider, filters) =>
      filters[0]?.value === '/first.md' ? first : answer('/second.md', '# Second'),
    ),
    cancel,
    publish,
    fail: vi.fn(),
  });

  const retired = controller.select(source, answer('/first.md').rows[0]!);
  await controller.select(source, answer('/second.md').rows[0]!);
  releaseFirst(answer('/first.md', '# First'));
  await retired;

  expect(cancel).toHaveBeenCalledOnce();
  expect(publish).toHaveBeenCalledOnce();
  expect(publish.mock.calls[0]![0].row.record_key).toBe('/second.md');
});

it('surfaces a typed point-read not-found and never publishes cloud ownership', async () => {
  const publish = vi.fn();
  const fail = vi.fn();
  const controller = createCloudSelectionController({
    query: vi.fn(async () => {
      throw new SettingsCommandError('cloud', {
        kind: 'query-not-found',
        source_id: source.id,
        record_key: '/missing.md',
      });
    }),
    cancel: vi.fn(async () => false),
    publish,
    fail,
  });

  await controller.select(source, answer('/missing.md').rows[0]!);

  expect(publish).not.toHaveBeenCalled();
  expect(fail).toHaveBeenCalledWith(
    'The selected provider document was not found. Refresh the Library and try again.',
  );
});
