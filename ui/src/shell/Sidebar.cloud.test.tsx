import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';

import type {
  LibraryEntityQueryResponse_Serialize,
  LibrarySourceExecution,
} from '../bindings.js';
import { Sidebar } from './Sidebar.js';

afterEach(cleanup);

it('navigates connected read-through sources from the primary Library rail', () => {
  const member: LibrarySourceExecution = {
    membership_id: '00000000-0000-4000-8000-000000000011',
    registry_id: 'dropbox',
    name: 'Dropbox',
    kind: 'dropbox',
    source_ids: ['00000000-0000-4000-8000-000000000012'],
    editable: false,
    active: false,
  };
  const response: LibraryEntityQueryResponse_Serialize = {
    memberships: [member],
    answer: {
      query_id: '00000000-0000-4000-8000-000000000013',
      pins: {
        catalog_resolver_id: '00000000-0000-4000-8000-000000000014',
        catalog_revision_id: '00000000-0000-4000-8000-000000000015',
        schema_version_id: '00000000-0000-4000-8000-000000000016',
        physical_schema_version_ids: ['00000000-0000-4000-8000-000000000017'],
        mapping_version_ids: ['00000000-0000-4000-8000-000000000018'],
        source_version_ids: ['00000000-0000-4000-8000-000000000019'],
        policy_version_ids: ['00000000-0000-4000-8000-000000000020'],
        provenance_rule_version_id: '00000000-0000-4000-8000-000000000031',
      },
      completeness: { state: 'complete' },
      sources: [
        {
          source_id: member.source_ids[0]!,
          source_version_id: '00000000-0000-4000-8000-000000000019',
          freshness_ms: 0,
          outcome: 'ran',
          retry_after_ms: null,
          scanned: 1,
          returned: 1,
        },
      ],
      rows: [
        {
          record_key: '/notes/cloud.md',
          fields: { title: 'Cloud note.md', path: '/notes/cloud.md' },
          provenance: {
            path: {
              field: 'path',
              provenance_rule_version_id: '00000000-0000-4000-8000-000000000031',
              selected: 0,
              considered: [
                {
                  source_version_id: '00000000-0000-4000-8000-000000000019',
                  record_key: '/notes/cloud.md',
                  mapping_version_id: '00000000-0000-4000-8000-000000000018',
                  physical_attribute: 'path',
                  observed_at_ms: 1,
                  state: { state: 'observed_value', value: '/notes/cloud.md' },
                },
              ],
            },
          },
        },
      ],
      next_cursor: null,
    },
  };
  const open = vi.fn();
  render(
    <Sidebar
      collapsed={false}
      root={null}
      files={[]}
      recent={[]}
      activePath={null}
      loading={false}
      onPickFolder={vi.fn()}
      onOpenFile={vi.fn()}
      onOpenRecent={vi.fn()}
      libraryEvidence={[response]}
      librarySearch=""
      onLibrarySearch={vi.fn()}
      onOpenLibrary={open}
      onRefreshLibrary={vi.fn()}
    />,
  );

  expect(screen.getByRole('navigation', { name: 'Library sources' })).toBeTruthy();
  expect(screen.getByText('Read-only')).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: 'Open Cloud note.md from Dropbox' }));
  expect(open).toHaveBeenCalledWith(member, response.answer.rows[0]);
});

it('lists a local membership from the same immutable Library answer', () => {
  const member: LibrarySourceExecution = {
    membership_id: '00000000-0000-4000-8000-000000000021',
    registry_id: 'local-source-id',
    name: 'Local notes',
    kind: 'local-folder',
    source_ids: ['00000000-0000-4000-8000-000000000022'],
    editable: true,
    active: true,
  };
  const response: LibraryEntityQueryResponse_Serialize = {
    memberships: [member],
    answer: {
      query_id: '00000000-0000-4000-8000-000000000023',
      pins: {
        catalog_resolver_id: '00000000-0000-4000-8000-000000000024',
        catalog_revision_id: '00000000-0000-4000-8000-000000000025',
        schema_version_id: '00000000-0000-4000-8000-000000000026',
        physical_schema_version_ids: ['00000000-0000-4000-8000-000000000027'],
        mapping_version_ids: ['00000000-0000-4000-8000-000000000028'],
        source_version_ids: ['00000000-0000-4000-8000-000000000029'],
        policy_version_ids: ['00000000-0000-4000-8000-000000000030'],
        provenance_rule_version_id: '00000000-0000-4000-8000-000000000032',
      },
      completeness: { state: 'complete' },
      sources: [
        {
          source_id: member.source_ids[0]!,
          source_version_id: '00000000-0000-4000-8000-000000000029',
          freshness_ms: 0,
          outcome: 'ran',
          retry_after_ms: null,
          scanned: 1,
          returned: 1,
        },
      ],
      rows: [
        {
          record_key:
            'library:00000000-0000-4000-8000-000000000021:notes/local.md',
          fields: {
            title: 'Local note',
            path: 'notes/local.md',
            body: '# Local note',
          },
          provenance: {
            path: {
              field: 'path',
              provenance_rule_version_id: '00000000-0000-4000-8000-000000000032',
              selected: 0,
              considered: [
                {
                  source_version_id: '00000000-0000-4000-8000-000000000029',
                  record_key: 'notes/local.md',
                  mapping_version_id: '00000000-0000-4000-8000-000000000028',
                  physical_attribute: 'path',
                  observed_at_ms: 1,
                  state: { state: 'observed_value', value: 'notes/local.md' },
                },
              ],
            },
          },
        },
      ],
      next_cursor: null,
    },
  };
  const open = vi.fn();
  render(
    <Sidebar
      collapsed={false}
      root="/Library/Local notes"
      files={[]}
      recent={[]}
      activePath={null}
      loading={false}
      onPickFolder={vi.fn()}
      onOpenFile={vi.fn()}
      onOpenRecent={vi.fn()}
      libraryEvidence={[response]}
      librarySearch=""
      onLibrarySearch={vi.fn()}
      onOpenLibrary={open}
      onRefreshLibrary={vi.fn()}
    />,
  );

  expect(screen.getByRole('navigation', { name: 'Library sources' })).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: 'Open Local note from Local notes' }));
  expect(open).toHaveBeenCalledWith(member, response.answer.rows[0]);
});
