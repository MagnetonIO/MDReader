import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';

import type {
  CloudSourceDto,
  LibraryEntityQueryResponse_Serialize,
  LibrarySourceExecution,
} from '../bindings.js';
import { Sidebar } from './Sidebar.js';

afterEach(cleanup);

it('navigates connected read-through sources from the primary Library rail', () => {
  const source: CloudSourceDto = {
    id: 'a'.repeat(64),
    provider: 'dropbox',
    label: 'Dropbox',
    connection: 'connected',
    enabled: true,
  };
  const member: LibrarySourceExecution = {
    membership_id: '00000000-0000-4000-8000-000000000011',
    name: 'Dropbox',
    kind: 'dropbox',
    source_ids: ['00000000-0000-4000-8000-000000000012'],
    editable: false,
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
      },
      completeness: { state: 'complete' },
      sources: [
        {
          source_id: member.source_ids[0]!,
          source_version_id: '00000000-0000-4000-8000-000000000019',
          freshness_ms: 0,
          outcome: 'ran',
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
      cloudSources={[source]}
      libraryEvidence={response}
      cloudSearch=""
      onCloudSearch={vi.fn()}
      onOpenCloud={open}
      onRefreshCloud={vi.fn()}
    />,
  );

  expect(screen.getByRole('navigation', { name: 'Connected Library sources' })).toBeTruthy();
  expect(screen.getByText('Read-through · not locally indexed')).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: 'Open Cloud note.md from Dropbox' }));
  expect(open).toHaveBeenCalledWith(source, member, response.answer.rows[0]);
});
