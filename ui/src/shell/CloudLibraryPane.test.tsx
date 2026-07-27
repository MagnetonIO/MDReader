import { cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';

import type {
  CloudDocumentQueryResponse,
  CloudFilterInput,
  CloudProvider,
  CloudSourceDto,
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
    catalog_version: 'catalog-v7',
    schema_version: 'schema-v7',
    schema_checksum: 'b'.repeat(64),
    source_schema_version: 'drive-schema-v1',
    source_schema_checksum: 'c'.repeat(64),
    mapping_version: 'mapping-v1',
    mapping_checksum: 'd'.repeat(64),
    connector_version: 'google-drive/v1',
    source_version: 'source-v1',
    source_checksum: 'e'.repeat(64),
    policy_version: 'policy-v1',
    policy_checksum: 'f'.repeat(64),
  },
  completeness: 'complete',
  scanned: 1,
  sources: [
    {
      source_id: source.id,
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
            observation: 'observed' as const,
            observed_at_ms: 1_800_000_000_000,
          },
        ],
      })),
    },
  ],
};

afterEach(cleanup);

it('reads a provider-owned document in the main Library with semantic evidence', async () => {
  const query = vi.fn(async () => response);
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
  expect(
    screen.getAllByText(
      'Provider google-drive · record drive-file-1 · mapping mapping-v1 · observed at 2027-01-15T08:00:00.000Z',
    ),
  ).toHaveLength(3);

  fireEvent.click(screen.getByRole('tab', { name: 'Schema' }));
  expect(screen.getByText('library · vschema-v7')).toBeTruthy();
  expect(screen.getByText(/read-only external source evidence/i)).toBeTruthy();
  expect(screen.getByText(/not the provider-owned document/i)).toBeTruthy();
  expect(screen.queryByText(/filesystem, Git, and frontmatter evidence/i)).toBeNull();
  expect(query).toHaveBeenCalledWith(
    'google-drive',
    [{ field: 'path', operator: 'eq', value: 'drive-file-1' }],
    expect.arrayContaining(['path', 'title', 'body']),
    expect.stringContaining('document-inspector-'),
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
      cancel={cancel}
    />,
  );
  rendered.rerender(
    <CloudLibraryPane
      source={source}
      row={{ ...current.rows[0]!, fields: { title: 'Current.md', path: 'drive-file-2' } }}
      query={query}
      cancel={cancel}
    />,
  );

  expect(await screen.findByText('Current provider body.')).toBeTruthy();
  releaseFirst(response);
  await Promise.resolve();
  expect(screen.queryByText('Provider-owned body.')).toBeNull();
  expect(cancel).toHaveBeenCalledWith(expect.stringContaining('drive-file-1'));
});
