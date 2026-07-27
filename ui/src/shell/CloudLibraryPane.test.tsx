import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';

import type {
  CloudDocumentQueryResponse,
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
  render(
    <CloudLibraryPane
      source={source}
      row={response.rows[0]!}
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

  fireEvent.click(screen.getByRole('tab', { name: 'Provenance' }));
  expect(await screen.findByText(`${source.id}.body`)).toBeTruthy();
  fireEvent.click(screen.getByRole('tab', { name: 'Schema' }));
  expect(screen.getByText('library · vschema-v7')).toBeTruthy();
  expect(query).toHaveBeenCalledWith(
    'google-drive',
    [{ field: 'path', operator: 'eq', value: 'drive-file-1' }],
    expect.arrayContaining(['path', 'title', 'body']),
    expect.stringContaining('document-inspector-'),
  );
});
