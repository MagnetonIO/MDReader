import { cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';

import type {
  CloudSourceDto,
  EntityRow,
  LibraryEntityQueryResponse_Serialize,
} from '../bindings.js';
import { CloudLibraryPane } from './CloudLibraryPane.js';

const source: CloudSourceDto = {
  id: 'a'.repeat(64),
  provider: 'google-drive',
  label: 'Google Drive',
  connection: 'connected',
  enabled: true,
};

const SOURCE_VERSION = '00000000-0000-4000-8000-000000000007';
const MAPPING_VERSION = '00000000-0000-4000-8000-000000000006';

const row: EntityRow = {
  record_key: 'drive-file-1',
  fields: {
    path: 'drive-file-1',
    title: 'Cloud note.md',
    body: '# Cloud note\n\nProvider-owned body.',
  },
  provenance: Object.fromEntries(
    [
      ['path', 'id', 'drive-file-1'],
      ['title', 'name', 'Cloud note.md'],
      ['body', 'body', '# Cloud note\n\nProvider-owned body.'],
    ].map(([field, physicalAttribute, value]) => [
      field,
      {
        field,
        selected: 0,
        considered: [
          {
            source_version_id: SOURCE_VERSION,
            record_key: 'drive-file-1',
            mapping_version_id: MAPPING_VERSION,
            physical_attribute: physicalAttribute,
            observed_at_ms: 1_800_000_000_000,
            state: { state: 'observed_value' as const, value },
          },
        ],
      },
    ]),
  ),
};

const response: LibraryEntityQueryResponse_Serialize = {
  answer: {
    query_id: '00000000-0000-4000-8000-000000000001',
    pins: {
      catalog_resolver_id: '00000000-0000-4000-8000-000000000002',
      catalog_revision_id: '00000000-0000-4000-8000-000000000003',
      schema_version_id: '00000000-0000-4000-8000-000000000004',
      physical_schema_version_ids: ['00000000-0000-4000-8000-000000000005'],
      mapping_version_ids: [MAPPING_VERSION],
      source_version_ids: [SOURCE_VERSION],
      policy_version_ids: ['00000000-0000-4000-8000-000000000008'],
    },
    rows: [row],
    sources: [
      {
        source_id: source.id,
        source_version_id: SOURCE_VERSION,
        outcome: 'ran',
        freshness_ms: 0,
        scanned: 1,
        returned: 1,
      },
    ],
    completeness: { state: 'complete' },
    next_cursor: null,
  },
  memberships: [
    {
      membership_id: '00000000-0000-4000-8000-000000000010',
      name: 'Google Drive',
      kind: 'google-drive',
      source_ids: [source.id],
      editable: false,
    },
  ],
};

afterEach(cleanup);

it('reads and inspects one hydrated provider response without a second query', async () => {
  render(
    <CloudLibraryPane
      source={source}
      row={row}
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
  expect(
    screen.queryByRole('complementary', { name: 'Semantic Inspector' }),
  ).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: 'Inspect' }));
  const inspector = within(
    screen.getByRole('complementary', { name: 'Semantic Inspector' }),
  );
  expect(inspector.getByText('Complete')).toBeTruthy();
  expect(inspector.getByText('Cloud note.md')).toBeTruthy();

  fireEvent.click(screen.getByRole('tab', { name: 'Sources' }));
  expect(screen.getByText(source.id)).toBeTruthy();
  expect(screen.getByText('0 ms freshness')).toBeTruthy();
  expect(screen.getByText('1 scanned · 1 returned')).toBeTruthy();

  fireEvent.click(screen.getByRole('tab', { name: 'Provenance' }));
  expect(await screen.findByText(`${SOURCE_VERSION}.body`)).toBeTruthy();
  expect(screen.getByText(`${SOURCE_VERSION}.name`)).toBeTruthy();
  expect(screen.getByText(`${SOURCE_VERSION}.id`)).toBeTruthy();
  expect(screen.queryByText(`${SOURCE_VERSION}.title`)).toBeNull();
  expect(
    screen.getAllByText(
      'record drive-file-1 · mapping 00000000-0000-4000-8000-000000000006',
    ),
  ).toHaveLength(3);

  fireEvent.click(screen.getByRole('tab', { name: 'Schema' }));
  expect(screen.getByText('00000000-0000-4000-8000-000000000003')).toBeTruthy();
  expect(screen.getByText('00000000-0000-4000-8000-000000000004')).toBeTruthy();
  expect(screen.getByText('00000000-0000-4000-8000-000000000005')).toBeTruthy();
  expect(screen.getByText('00000000-0000-4000-8000-000000000006')).toBeTruthy();
  expect(screen.getByText('00000000-0000-4000-8000-000000000007')).toBeTruthy();
  expect(screen.getByText('00000000-0000-4000-8000-000000000008')).toBeTruthy();
  expect(screen.queryByText('Attributes')).toBeNull();
  expect(screen.queryByText(/read-only external source evidence/i)).toBeNull();
});

it('renders backend-authored null, absence, unobservable reason, reports, and pins exactly', () => {
  const exactRow = {
    ...row,
    fields: {
      path: 'drive-file-1',
      title: null,
      body: '# Null title',
    },
    provenance: {
      title: {
        field: 'title',
        selected: 0,
        considered: [
          {
            ...row.provenance.path!.considered[0]!,
            physical_attribute: 'name',
            state: { state: 'observed_null' as const },
          },
        ],
      },
      revision: {
        field: 'revision',
        selected: null,
        considered: [
          {
            ...row.provenance.path!.considered[0]!,
            physical_attribute: 'revisionId',
            state: { state: 'field_unseen' as const },
          },
        ],
      },
      author: {
        field: 'author',
        selected: null,
        considered: [
          {
            ...row.provenance.path!.considered[0]!,
            physical_attribute: null,
            state: {
              state: 'not_asked' as const,
              value: { reason: 'pinned mapping has no author attribute' },
            },
            observed_at_ms: null,
          },
        ],
      },
      body: row.provenance.body!,
    },
  };
  const exactResponse: LibraryEntityQueryResponse_Serialize = {
    ...response,
    answer: {
      ...response.answer,
      sources: [
        {
          source_id: source.id,
          source_version_id: SOURCE_VERSION,
          freshness_ms: 4_321,
          outcome: 'ran',
          scanned: 19,
          returned: 1,
        },
      ],
      rows: [exactRow],
    },
  };

  render(
    <CloudLibraryPane
      source={source}
      row={exactRow}
      evidence={exactResponse}
    />,
  );

  fireEvent.click(screen.getByRole('button', { name: 'Inspect' }));
  const inspector = within(
    screen.getByRole('complementary', { name: 'Semantic Inspector' }),
  );
  expect(inspector.getByText('null')).toBeTruthy();
  expect(inspector.getAllByText('No selected provider')).toHaveLength(2);

  fireEvent.click(inspector.getByRole('tab', { name: 'Sources' }));
  expect(inspector.getByText('4321 ms freshness')).toBeTruthy();
  expect(inspector.getByText('19 scanned · 1 returned')).toBeTruthy();

  fireEvent.click(inspector.getByRole('tab', { name: 'Provenance' }));
  expect(inspector.getByText('observed null')).toBeTruthy();
  expect(inspector.getByText('field not observed')).toBeTruthy();
  expect(inspector.getByText('not asked · pinned mapping has no author attribute')).toBeTruthy();
  expect(inspector.queryByText('record not observed')).toBeNull();

  fireEvent.click(inspector.getByRole('tab', { name: 'Schema' }));
  expect(inspector.getByText(SOURCE_VERSION)).toBeTruthy();
});

it('never mounts Semantic Inspector for a row that was not body-hydrated', () => {
  const unhydrated = {
    ...row,
    fields: { path: 'drive-file-1', title: 'Unavailable.md' },
  };
  render(
    <CloudLibraryPane
      source={source}
      row={unhydrated}
      evidence={{ ...response, answer: { ...response.answer, rows: [unhydrated] } }}
    />,
  );

  expect(screen.getByText(/provider body is unavailable/i)).toBeTruthy();
  expect(
    screen.queryByRole('complementary', { name: 'Semantic Inspector' }),
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
  const answer: LibraryEntityQueryResponse_Serialize = {
    ...response,
    answer: {
      ...response.answer,
      rows: [
        {
          ...row,
          record_key: recordKey,
          fields: { path: recordKey, title: 'Long path.md', body: '# Long path' },
          provenance: Object.fromEntries(
            Object.entries(row.provenance).map(([field, evidence]) => [
              field,
              {
                ...evidence,
                considered: evidence.considered.map((provider) => ({
                  ...provider,
                  record_key: recordKey,
                  state:
                    field === 'path'
                      ? { state: 'observed_value' as const, value: recordKey }
                      : provider.state,
                })),
              },
            ]),
          ),
        },
      ],
    },
  };
  render(
    <CloudLibraryPane
      source={dropboxSource}
      row={answer.answer.rows[0]!}
      evidence={answer}
    />,
  );

  expect(await screen.findByText('Long path')).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: 'Inspect' }));
  const inspector = within(
    screen.getByRole('complementary', { name: 'Semantic Inspector' }),
  );
  expect(inspector.getAllByText(recordKey)).toHaveLength(2);
});
