import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';

import type { CloudDocumentQueryResponse, CloudSourceDto } from '../bindings.js';
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
  const response: CloudDocumentQueryResponse = {
    query_id: 'sidebar-query',
    provider: 'dropbox',
    workspace_id: 'workspace-v1',
    schema_version: 'schema-v1',
    version_pins: {
      catalog_version: 'catalog-v1',
      schema_version: 'schema-v1',
      schema_checksum: 'b'.repeat(64),
      source_schema_version: 'source-schema-v1',
      source_schema_checksum: 'c'.repeat(64),
      mapping_version: 'mapping-v1',
      mapping_checksum: 'd'.repeat(64),
      connector_version: 'dropbox/v1',
      source_version: 'source-v1',
      source_checksum: 'e'.repeat(64),
      policy_version: 'policy-v1',
      policy_checksum: 'f'.repeat(64),
    },
    completeness: 'complete',
    sources: [
    {
      source_id: source.id,
      freshness_ms: 0,
      outcome: {
          kind: 'ran',
          mode: 'virtual',
          scanned: 1,
          returned: 1,
          pushed: [],
          residual: [],
        },
      },
    ],
    scanned: 1,
    rows: [
      {
        source_id: source.id,
        record_key: '/notes/cloud.md',
        fields: { title: 'Cloud note.md', path: '/notes/cloud.md' },
        provenance: [],
      },
    ],
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
      cloudDocuments={{ dropbox: response }}
      cloudSearch=""
      onCloudSearch={vi.fn()}
      onOpenCloud={open}
      onRefreshCloud={vi.fn()}
    />,
  );

  expect(screen.getByRole('navigation', { name: 'Connected Library sources' })).toBeTruthy();
  expect(screen.getByText('Read-through · not locally indexed')).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: 'Open Cloud note.md from Dropbox' }));
  expect(open).toHaveBeenCalledWith(source, response, response.rows[0]);
});
