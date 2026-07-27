import { cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';

import type {
  CloudDocumentQueryResponse,
  CloudFilterInput,
  CloudProvider,
  CloudSourceDto,
  EntityQueryResponse,
} from '../bindings.js';
import { CloudLibraryPane } from './CloudLibraryPane.js';

const source: CloudSourceDto = {
  id: 'a'.repeat(64),
  provider: 'google-drive',
  label: 'Google Drive',
  connection: 'connected',
  enabled: true,
};

const response: CloudDocumentQueryResponse = {
  query_id: 'query-1',
  provider: 'google-drive',
  schema_version: 'schema-v7',
  version_pins: {
    catalog_version: '00000000-0000-4000-8000-000000000003',
    schema_version: '00000000-0000-4000-8000-000000000004',
    schema_checksum: 'b'.repeat(64),
    source_schema_version: '00000000-0000-4000-8000-000000000005',
    source_schema_checksum: 'c'.repeat(64),
    mapping_version: '00000000-0000-4000-8000-000000000006',
    mapping_checksum: 'd'.repeat(64),
    connector_version: 'google-drive/v1',
    source_version: '00000000-0000-4000-8000-000000000007',
    source_checksum: 'e'.repeat(64),
    policy_version: '00000000-0000-4000-8000-000000000008',
    policy_checksum: 'f'.repeat(64),
  },
  completeness: 'complete',
  scanned: 1,
  sources: [
    {
      source_id: source.id,
      freshness_ms: 0,
      outcome: {
        kind: 'ran',
        mode: 'virtual',
        scanned: 1,
        returned: 1,
        pushed: ['path'],
        residual: [],
      },
    },
  ],
  rows: [
    {
      source_id: source.id,
      record_key: 'drive-file-1',
      fields: {
        path: 'drive-file-1',
        title: 'Cloud note.md',
        body: '# Cloud note\n\nProvider-owned body.',
      },
      provenance: ['path', 'title', 'body'].map((field) => ({
        field,
        providers: [
          {
            provider: 'google-drive' as const,
            source_id: source.id,
            record_key: 'drive-file-1',
            mapping_version: 'mapping-v1',
            physical_attribute:
              field === 'path' ? 'id' : field === 'title' ? 'name' : field,
            observation: 'observed' as const,
            observed_at_ms: 1_800_000_000_000,
          },
        ],
      })),
    },
  ],
};

function entityResponse(
  cloud: CloudDocumentQueryResponse = response,
): EntityQueryResponse {
  return {
    query_id: '00000000-0000-4000-8000-000000000001',
    pins: {
      catalog_resolver_id: '00000000-0000-4000-8000-000000000002',
      catalog_revision_id: '00000000-0000-4000-8000-000000000003',
      schema_version_id: '00000000-0000-4000-8000-000000000004',
      physical_schema_version_ids: ['00000000-0000-4000-8000-000000000005'],
      mapping_version_ids: ['00000000-0000-4000-8000-000000000006'],
      source_version_ids: ['00000000-0000-4000-8000-000000000007'],
      policy_version_ids: ['00000000-0000-4000-8000-000000000008'],
    },
    rows: cloud.rows.map((row) => ({
      record_key: row.record_key,
      fields: row.fields as Record<string, unknown>,
      provenance: Object.fromEntries(
        row.provenance.map((field) => [
          field.field,
          {
            field: field.field,
            selected: 0,
            considered: field.providers.map((provider) => ({
              source_version_id: '00000000-0000-4000-8000-000000000007',
              record_key: provider.record_key,
              mapping_version_id: '00000000-0000-4000-8000-000000000006',
              physical_attribute: provider.physical_attribute,
              observed_at_ms: provider.observed_at_ms,
              state: {
                state: 'observed_value' as const,
                value: (row.fields as Record<string, unknown>)[field.field],
              },
            })),
          },
        ]),
      ),
    })),
    sources: [
      {
        source_id: '00000000-0000-4000-8000-000000000009',
        source_version_id: '00000000-0000-4000-8000-000000000007',
        outcome: 'ran',
        freshness_ms: 0,
        scanned: 1,
        returned: 1,
      },
    ],
    completeness: { state: 'complete' },
    next_cursor: null,
  };
}

afterEach(cleanup);

it('reads a provider-owned document in the main Library with semantic evidence', async () => {
  const query = vi.fn(async () => response);
  const sharedQuery = vi.fn(async () => entityResponse());
  const listingRow = {
    ...response.rows[0]!,
    fields: {
      path: 'drive-file-1',
      title: 'Cloud note.md',
    },
    provenance: response.rows[0]!.provenance.filter((field) => field.field !== 'body'),
  };
  render(
    <CloudLibraryPane
      source={source}
      row={listingRow}
      query={query}
      entityQuery={sharedQuery}
      cancel={vi.fn(async () => false)}
    />,
  );

  expect(await screen.findByText('Cloud note')).toBeTruthy();
  expect(screen.getByText('Provider-owned body.')).toBeTruthy();
  expect(screen.getByText(/Read-only/)).toBeTruthy();
  expect(screen.getByText(/not copied into the local index/i)).toBeTruthy();
  expect(screen.getByRole('link', { name: 'Open original' }).getAttribute('href')).toContain(
    'drive-file-1',
  );
  const inspector = within(
    screen.getByRole('complementary', { name: 'Cloud Semantic Inspector' }),
  );
  expect(inspector.getByText('Complete')).toBeTruthy();
  expect(inspector.getByText('Cloud note.md')).toBeTruthy();

  fireEvent.click(screen.getByRole('tab', { name: 'Sources' }));
  expect(screen.getByText(source.id)).toBeTruthy();
  expect(screen.getByText('virtual · live')).toBeTruthy();
  expect(screen.getByText('1 scanned · 1 returned')).toBeTruthy();

  fireEvent.click(screen.getByRole('tab', { name: 'Provenance' }));
  expect(await screen.findByText(`${source.id}.body`)).toBeTruthy();
  expect(screen.getByText(`${source.id}.name`)).toBeTruthy();
  expect(screen.getByText(`${source.id}.id`)).toBeTruthy();
  expect(screen.queryByText(`${source.id}.title`)).toBeNull();
  expect(
    screen.getAllByText(
      'Provider google-drive · record drive-file-1 · mapping 00000000-0000-4000-8000-000000000006 · observed at 2027-01-15T08:00:00.000Z',
    ),
  ).toHaveLength(3);

  fireEvent.click(screen.getByRole('tab', { name: 'Schema' }));
  expect(
    screen.getByText('document · v00000000-0000-4000-8000-000000000004'),
  ).toBeTruthy();
  expect(screen.getByText(/read-only external source evidence/i)).toBeTruthy();
  expect(screen.getByText(/not the provider-owned document/i)).toBeTruthy();
  expect(screen.queryByText(/filesystem, Git, and frontmatter evidence/i)).toBeNull();
  expect(sharedQuery).toHaveBeenCalledWith(
    'google-drive',
    [{ field: 'path', operator: 'eq', value: 'drive-file-1' }],
    expect.arrayContaining(['path', 'title', 'body']),
    expect.stringContaining('document-inspector-'),
  );
  expect(query).toHaveBeenCalledWith(
    'google-drive',
    [{ field: 'path', operator: 'eq', value: 'drive-file-1' }],
    expect.arrayContaining(['path', 'title', 'body']),
    expect.stringContaining('cloud-reader:google-drive:'),
  );
});

it('retires a delayed body read when another cloud document replaces it', async () => {
  let releaseFirst!: (answer: CloudDocumentQueryResponse) => void;
  const first = new Promise<CloudDocumentQueryResponse>((resolve) => {
    releaseFirst = resolve;
  });
  const current = {
    ...response,
    query_id: 'query-current',
    rows: [
      {
        ...response.rows[0]!,
        record_key: 'drive-file-2',
        fields: {
          path: 'drive-file-2',
          title: 'Current.md',
          body: '# Current\n\nCurrent provider body.',
        },
      },
    ],
  };
  const query = vi
    .fn<
      (
        provider: CloudProvider,
        filters: CloudFilterInput[],
        fields: string[],
        requestId: string,
      ) => Promise<CloudDocumentQueryResponse>
    >()
    .mockImplementation((_provider, filters) =>
      filters[0]?.value === 'drive-file-1' ? first : Promise.resolve(current),
    );
  const cancel = vi.fn(async () => true);
  const rendered = render(
    <CloudLibraryPane
      source={source}
      row={{ ...response.rows[0]!, fields: { title: 'Retired.md', path: 'drive-file-1' } }}
      query={query}
      entityQuery={vi.fn(async () => entityResponse())}
      cancel={cancel}
    />,
  );
  const retiredBodyRequestId = query.mock.calls.find((call) =>
    call[3].startsWith('cloud-reader:'),
  )?.[3];
  expect(retiredBodyRequestId).toBeTruthy();
  rendered.rerender(
    <CloudLibraryPane
      source={source}
      row={{ ...current.rows[0]!, fields: { title: 'Current.md', path: 'drive-file-2' } }}
      query={query}
      entityQuery={vi.fn(async () => entityResponse(current))}
      cancel={cancel}
    />,
  );

  expect(await screen.findByText('Current provider body.')).toBeTruthy();
  releaseFirst(response);
  await Promise.resolve();
  expect(screen.queryByText('Provider-owned body.')).toBeNull();
  expect(cancel).toHaveBeenCalledWith(retiredBodyRequestId);
  expect(retiredBodyRequestId).not.toContain('drive-file-1');
});

it('keeps request IDs bounded when a valid Dropbox record key is 4 KiB', async () => {
  const recordKey = `/${'a'.repeat(4092)}.md`;
  expect(new TextEncoder().encode(recordKey)).toHaveLength(4_096);
  const dropboxSource: CloudSourceDto = {
    ...source,
    provider: 'dropbox',
    label: 'Dropbox',
  };
  const answer: CloudDocumentQueryResponse = {
    ...response,
    provider: 'dropbox',
    rows: [
      {
        ...response.rows[0]!,
        record_key: recordKey,
        fields: { path: recordKey, title: 'Long path.md', body: '# Long path' },
        provenance: response.rows[0]!.provenance.map((field) => ({
          ...field,
          providers: field.providers.map((provider) => ({
            ...provider,
            provider: 'dropbox' as const,
            record_key: recordKey,
          })),
        })),
      },
    ],
  };
  const query = vi.fn<
    (
      provider: CloudProvider,
      filters: CloudFilterInput[],
      fields: string[],
      requestId: string,
    ) => Promise<CloudDocumentQueryResponse>
  >(async () => answer);
  render(
    <CloudLibraryPane
      source={dropboxSource}
      row={{ ...answer.rows[0]!, fields: { path: recordKey, title: 'Long path.md' } }}
      query={query}
      entityQuery={vi.fn(async () => entityResponse(answer))}
      cancel={vi.fn(async () => false)}
    />,
  );

  expect(await screen.findByText('Long path')).toBeTruthy();
  expect(query.mock.calls.length).toBeGreaterThan(0);
  for (const [provider, filters, , requestId] of query.mock.calls) {
    expect(provider).toBe('dropbox');
    expect(filters[0]?.value).toBe(recordKey);
    expect(requestId.length).toBeLessThanOrEqual(128);
    expect(requestId).not.toContain(recordKey);
  }
});
