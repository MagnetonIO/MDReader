import { describe, expect, it, vi } from 'vitest';

import type {
  CloudSourceDto,
  EntityRow,
  LibraryEntityQueryResponse_Serialize,
  LibrarySourceExecution,
} from '../bindings.js';
import {
  hydrateLibrary,
  memberForRow,
  rowsForMember,
} from './library-documents.js';

const PHYSICAL_SOURCE_ID = '00000000-0000-4000-8000-000000000011';
const SOURCE_VERSION_ID = '00000000-0000-4000-8000-000000000012';
const MEMBERSHIP_ID = '00000000-0000-4000-8000-000000000013';

const row: EntityRow = {
  record_key: 'drive-file',
  fields: { path: 'drive-file', title: 'One answer.md', body: '# One answer' },
  provenance: {
    path: {
      field: 'path',
      selected: 0,
      considered: [
        {
          source_version_id: SOURCE_VERSION_ID,
          record_key: 'drive-file',
          mapping_version_id: '00000000-0000-4000-8000-000000000014',
          physical_attribute: 'id',
          observed_at_ms: 1,
          state: { state: 'observed_value', value: 'drive-file' },
        },
      ],
    },
    title: {
      field: 'title',
      selected: 0,
      considered: [
        {
          source_version_id: SOURCE_VERSION_ID,
          record_key: 'drive-file',
          mapping_version_id: '00000000-0000-4000-8000-000000000014',
          physical_attribute: 'name',
          observed_at_ms: 1,
          state: { state: 'observed_value', value: 'One answer.md' },
        },
      ],
    },
  },
};

const member: LibrarySourceExecution = {
  membership_id: MEMBERSHIP_ID,
  name: 'Google Drive',
  kind: 'google-drive',
  source_ids: [PHYSICAL_SOURCE_ID],
  editable: false,
};

const evidence: LibraryEntityQueryResponse_Serialize = {
  answer: {
    query_id: '00000000-0000-4000-8000-000000000001',
    pins: {
      catalog_resolver_id: '00000000-0000-4000-8000-000000000002',
      catalog_revision_id: '00000000-0000-4000-8000-000000000003',
      schema_version_id: '00000000-0000-4000-8000-000000000004',
      physical_schema_version_ids: ['00000000-0000-4000-8000-000000000005'],
      mapping_version_ids: ['00000000-0000-4000-8000-000000000014'],
      source_version_ids: [SOURCE_VERSION_ID],
      policy_version_ids: ['00000000-0000-4000-8000-000000000006'],
    },
    rows: [row],
    sources: [
      {
        source_id: PHYSICAL_SOURCE_ID,
        source_version_id: SOURCE_VERSION_ID,
        outcome: 'ran',
        freshness_ms: 0,
        scanned: 1,
        returned: 1,
      },
    ],
    completeness: { state: 'complete' },
    next_cursor: null,
  },
  memberships: [member],
};

const cloudSource: CloudSourceDto = {
  id: 'a'.repeat(64),
  provider: 'google-drive',
  label: 'Google Drive',
  connection: 'connected',
  enabled: true,
};

describe('one hydrated Library answer', () => {
  it('uses one entity-query IPC and never issues provider-specific reads', async () => {
    const queryLibraryDocuments = vi.fn(async () => evidence);
    const backend = {
      prepareCloudCatalog: vi.fn(async () => ({
        workspace_id: '00000000-0000-4000-8000-000000000020',
        catalog_revision_id: '00000000-0000-4000-8000-000000000021',
        schema_version_id: '00000000-0000-4000-8000-000000000022',
      })),
      listCloudSources: vi.fn(async () => ({
        version: 1,
        sources: [cloudSource],
      })),
      queryLibraryDocuments,
    };
    const result = await hydrateLibrary(backend, 'request-1');

    expect(result).toEqual({ sources: [cloudSource], evidence });
    expect(queryLibraryDocuments).toHaveBeenCalledOnce();
    expect(queryLibraryDocuments).toHaveBeenCalledWith(
      '00000000-0000-4000-8000-000000000020',
      [],
      [
        'providerId',
        'title',
        'author',
        'path',
        'mediaType',
        'modifiedAt',
        'size',
        'revision',
        'checksum',
        'body',
      ],
      'request-1',
    );
  });

  it('derives membership from selected provenance and keeps remote rows read-only', () => {
    expect(memberForRow(evidence, row)).toEqual(member);
    expect(rowsForMember(evidence, member)).toEqual([row]);
    expect(member.editable).toBe(false);
  });
});
