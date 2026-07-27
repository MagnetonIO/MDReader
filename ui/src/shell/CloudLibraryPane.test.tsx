import { cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';

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
  workspace_id: '00000000-0000-4000-8000-000000000010',
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
        selected: 0,
        providers: [
          {
            provider: 'google-drive' as const,
            source_id: source.id,
            record_key: 'drive-file-1',
            mapping_version: '00000000-0000-4000-8000-000000000006',
            physical_attribute:
              field === 'path' ? 'id' : field === 'title' ? 'name' : field,
            observation: {
              kind: 'observed-value' as const,
              value:
                field === 'path'
                  ? 'drive-file-1'
                  : field === 'title'
                    ? 'Cloud note.md'
                    : '# Cloud note\n\nProvider-owned body.',
            },
            observed_at_ms: 1_800_000_000_000,
          },
        ],
      })),
    },
  ],
};

afterEach(cleanup);

it('reads and inspects one hydrated provider response without a second query', async () => {
  render(
    <CloudLibraryPane
      source={source}
      row={response.rows[0]!}
      evidence={response}
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
  expect(screen.getByText('virtual · 0 ms freshness')).toBeTruthy();
  expect(screen.getByText('1 scanned · 1 returned')).toBeTruthy();

  fireEvent.click(screen.getByRole('tab', { name: 'Provenance' }));
  expect(await screen.findByText(`${source.id}.body`)).toBeTruthy();
  expect(screen.getByText(`${source.id}.name`)).toBeTruthy();
  expect(screen.getByText(`${source.id}.id`)).toBeTruthy();
  expect(screen.queryByText(`${source.id}.title`)).toBeNull();
  expect(
    screen.getAllByText(
      'provider google-drive · record drive-file-1 · mapping 00000000-0000-4000-8000-000000000006 · observed_at_ms 1800000000000',
    ),
  ).toHaveLength(3);

  fireEvent.click(screen.getByRole('tab', { name: 'Versions' }));
  expect(screen.getByText('00000000-0000-4000-8000-000000000003')).toBeTruthy();
  expect(screen.getByText('00000000-0000-4000-8000-000000000004')).toBeTruthy();
  expect(screen.getByText('b'.repeat(64))).toBeTruthy();
  expect(screen.getByText('00000000-0000-4000-8000-000000000005')).toBeTruthy();
  expect(screen.getByText('00000000-0000-4000-8000-000000000006')).toBeTruthy();
  expect(screen.getByText('00000000-0000-4000-8000-000000000007')).toBeTruthy();
  expect(screen.getByText('00000000-0000-4000-8000-000000000008')).toBeTruthy();
  expect(screen.queryByText('Attributes')).toBeNull();
  expect(screen.queryByText(/read-only external source evidence/i)).toBeNull();
});

it('renders backend-authored null, absence, unobservable reason, reports, and pins exactly', () => {
  const exactRow = {
    ...response.rows[0]!,
    fields: {
      path: 'drive-file-1',
      title: null,
      body: '# Null title',
    },
    provenance: [
      {
        field: 'title',
        selected: 0,
        providers: [
          {
            ...response.rows[0]!.provenance[0]!.providers[0]!,
            physical_attribute: 'name',
            observation: { kind: 'observed-null' as const },
          },
        ],
      },
      {
        field: 'revision',
        selected: null,
        providers: [
          {
            ...response.rows[0]!.provenance[0]!.providers[0]!,
            physical_attribute: 'revisionId',
            observation: {
              kind: 'observed-nothing' as const,
              reason: 'provider response omitted revisionId',
            },
          },
        ],
      },
      {
        field: 'author',
        selected: null,
        providers: [
          {
            ...response.rows[0]!.provenance[0]!.providers[0]!,
            physical_attribute: null,
            observation: {
              kind: 'unobservable' as const,
              reason: 'pinned mapping has no author attribute',
            },
            observed_at_ms: null,
          },
        ],
      },
      response.rows[0]!.provenance[2]!,
    ],
  };
  const exactResponse: CloudDocumentQueryResponse = {
    ...response,
    version_pins: {
      ...response.version_pins,
      connector_version: 'google-drive/exact-r9',
      policy_checksum: '9'.repeat(64),
    },
    sources: [
      {
        source_id: source.id,
        freshness_ms: 4_321,
        outcome: {
          kind: 'ran',
          mode: 'virtual',
          scanned: 19,
          returned: 1,
          pushed: ['path', 'title'],
          residual: ['body'],
        },
      },
    ],
    scanned: 19,
    rows: [exactRow],
  };

  render(
    <CloudLibraryPane
      source={source}
      row={exactRow}
      evidence={exactResponse}
    />,
  );

  const inspector = within(
    screen.getByRole('complementary', { name: 'Cloud Semantic Inspector' }),
  );
  expect(inspector.getByText('null')).toBeTruthy();
  expect(inspector.getAllByText('No selected provider')).toHaveLength(2);

  fireEvent.click(inspector.getByRole('tab', { name: 'Sources' }));
  expect(inspector.getByText('virtual · 4321 ms freshness')).toBeTruthy();
  expect(inspector.getByText('19 scanned · 1 returned')).toBeTruthy();
  expect(inspector.getByText('pushed path, title · residual body')).toBeTruthy();

  fireEvent.click(inspector.getByRole('tab', { name: 'Provenance' }));
  expect(inspector.getByText('observed null · selected')).toBeTruthy();
  expect(inspector.getByText('observed nothing')).toBeTruthy();
  expect(inspector.getByText('provider response omitted revisionId')).toBeTruthy();
  expect(inspector.getByText('unobservable')).toBeTruthy();
  expect(inspector.getByText('pinned mapping has no author attribute')).toBeTruthy();
  expect(inspector.queryByText('not consulted')).toBeNull();

  fireEvent.click(inspector.getByRole('tab', { name: 'Versions' }));
  expect(inspector.getByText('google-drive/exact-r9')).toBeTruthy();
  expect(inspector.getByText('9'.repeat(64))).toBeTruthy();
});

it('never mounts Semantic Inspector for a row that was not body-hydrated', () => {
  const unhydrated = {
    ...response.rows[0]!,
    fields: { path: 'drive-file-1', title: 'Unavailable.md' },
  };
  render(
    <CloudLibraryPane
      source={source}
      row={unhydrated}
      evidence={{ ...response, rows: [unhydrated] }}
    />,
  );

  expect(screen.getByText(/provider body is unavailable/i)).toBeTruthy();
  expect(
    screen.queryByRole('complementary', { name: 'Cloud Semantic Inspector' }),
  ).toBeNull();
});

it('preserves a valid 4 KiB Dropbox record key in the hydrated Inspector evidence', async () => {
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
            observation:
              field.field === 'path'
                ? { kind: 'observed-value' as const, value: recordKey }
                : provider.observation,
          })),
        })),
      },
    ],
  };
  render(
    <CloudLibraryPane
      source={dropboxSource}
      row={answer.rows[0]!}
      evidence={answer}
    />,
  );

  expect(await screen.findByText('Long path')).toBeTruthy();
  const inspector = within(
    screen.getByRole('complementary', { name: 'Cloud Semantic Inspector' }),
  );
  expect(inspector.getAllByText(recordKey)).toHaveLength(2);
});
