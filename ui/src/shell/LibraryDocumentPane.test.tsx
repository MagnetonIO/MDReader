import { cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';

import type {
  EntityRow,
  LibraryEntityQueryResponse_Serialize,
  LibrarySourceExecution,
} from '../bindings.js';
import { LibraryDocumentPane } from './LibraryDocumentPane.js';

const member: LibrarySourceExecution = {
  membership_id: '00000000-0000-4000-8000-000000000010',
  registry_id: 'google-drive',
  name: 'Google Drive',
  kind: 'google-drive',
  source_ids: ['a'.repeat(64)],
  editable: false,
  active: false,
};

const SOURCE_VERSION = '00000000-0000-4000-8000-000000000007';
const MAPPING_VERSION = '00000000-0000-4000-8000-000000000006';
const PROVENANCE_RULE_VERSION = '00000000-0000-4000-8000-000000000009';

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
        provenance_rule_version_id: PROVENANCE_RULE_VERSION,
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
      provenance_rule_version_id: PROVENANCE_RULE_VERSION,
    },
    rows: [row],
    sources: [
      {
        source_id: member.source_ids[0]!,
        source_version_id: SOURCE_VERSION,
        outcome: 'ran',
        retry_after_ms: null,
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
      ...member,
    },
  ],
};

afterEach(cleanup);

it('reads and inspects one hydrated provider response without a second query', async () => {
  render(
    <LibraryDocumentPane
      member={member}
      row={row}
      evidence={response}
    />,
  );

  expect(await screen.findByText('Cloud note')).toBeTruthy();
  expect(screen.getByText('Provider-owned body.')).toBeTruthy();
  expect(screen.getByText(/Read-only/)).toBeTruthy();
  expect(screen.getByText(/Changes remain in the source/i)).toBeTruthy();
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
  expect(screen.getByText(member.source_ids[0]!)).toBeTruthy();
  expect(screen.getByText('0 ms freshness')).toBeTruthy();
  expect(screen.getByText('1 scanned · 1 returned')).toBeTruthy();

  fireEvent.click(screen.getByRole('tab', { name: 'Provenance' }));
  expect(
    await screen.findByText(
      (_, element) => element?.textContent === `Google Drive · ${SOURCE_VERSION}.body`,
    ),
  ).toBeTruthy();
  expect(
    screen.getByText(
      (_, element) => element?.textContent === `Google Drive · ${SOURCE_VERSION}.name`,
    ),
  ).toBeTruthy();
  expect(
    screen.getByText(
      (_, element) => element?.textContent === `Google Drive · ${SOURCE_VERSION}.id`,
    ),
  ).toBeTruthy();
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

it('derives local edit capability from the selected Library membership', async () => {
  const edit = vi.fn();
  const localEvidence: LibraryEntityQueryResponse_Serialize = {
    ...response,
    memberships: [
      {
        ...response.memberships[0]!,
        registry_id: 'local-source-id',
        name: 'Local notes',
        kind: 'local-folder',
        editable: true,
        active: true,
      },
    ],
  };
  render(
    <LibraryDocumentPane
      member={localEvidence.memberships[0]!}
      row={row}
      evidence={localEvidence}
      onEdit={edit}
    />,
  );

  expect(await screen.findByText('Provider-owned body.')).toBeTruthy();
  expect(screen.queryByText(/Read-only/)).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: 'Edit local file' }));
  expect(edit).toHaveBeenCalledOnce();
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
        provenance_rule_version_id: PROVENANCE_RULE_VERSION,
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
        provenance_rule_version_id: PROVENANCE_RULE_VERSION,
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
        provenance_rule_version_id: PROVENANCE_RULE_VERSION,
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
          source_id: member.source_ids[0]!,
          source_version_id: SOURCE_VERSION,
          freshness_ms: 4_321,
          outcome: 'ran',
          retry_after_ms: null,
          scanned: 19,
          returned: 1,
        },
      ],
      rows: [exactRow],
    },
  };

  render(
    <LibraryDocumentPane
      member={member}
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

it('renders partial exclusions, retry guidance, and the provenance-rule pin', () => {
  const excludedSource = '00000000-0000-4000-8000-000000000099';
  const partial: LibraryEntityQueryResponse_Serialize = {
    ...response,
    answer: {
      ...response.answer,
      completeness: {
        state: 'partial',
        excluded_source_ids: [excludedSource],
      },
      sources: [
        ...response.answer.sources,
        {
          source_id: excludedSource,
          source_version_id: '00000000-0000-4000-8000-000000000098',
          outcome: 'rate_limited',
          retry_after_ms: 5_000,
          freshness_ms: null,
          scanned: 0,
          returned: 0,
        },
      ],
    },
    memberships: [
      {
        ...member,
        source_ids: [...member.source_ids, excludedSource],
      },
    ],
  };
  render(<LibraryDocumentPane member={partial.memberships[0]!} row={row} evidence={partial} />);

  fireEvent.click(screen.getByRole('button', { name: 'Inspect' }));
  const inspector = within(
    screen.getByRole('complementary', { name: 'Semantic Inspector' }),
  );
  expect(inspector.getByText('Partial')).toBeTruthy();
  expect(inspector.getByText(`Excluded source ${excludedSource}`)).toBeTruthy();

  fireEvent.click(inspector.getByRole('tab', { name: 'Sources' }));
  expect(inspector.getByText('Retry after 5000 ms')).toBeTruthy();

  fireEvent.click(inspector.getByRole('tab', { name: 'Provenance' }));
  expect(
    inspector.getAllByText(`provenance rule ${PROVENANCE_RULE_VERSION}`).length,
  ).toBeGreaterThan(0);

  fireEvent.click(inspector.getByRole('tab', { name: 'Schema' }));
  expect(inspector.getByText('Provenance rule pin')).toBeTruthy();
  expect(inspector.getByText(PROVENANCE_RULE_VERSION)).toBeTruthy();
});

it('never mounts Semantic Inspector for a row that was not body-hydrated', () => {
  const unhydrated = {
    ...row,
    fields: { path: 'drive-file-1', title: 'Unavailable.md' },
  };
  render(
    <LibraryDocumentPane
      member={member}
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
  const dropboxMember: LibrarySourceExecution = {
    ...member,
    registry_id: 'dropbox',
    name: 'Dropbox',
    kind: 'dropbox',
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
    <LibraryDocumentPane
      member={dropboxMember}
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
