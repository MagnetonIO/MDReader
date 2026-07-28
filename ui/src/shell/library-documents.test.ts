import { describe, expect, it, vi } from 'vitest';

import type {
  EntityRow,
  LibraryEntityQueryResponse_Serialize,
  LibrarySnapshot,
  LibrarySourceExecution,
} from '../bindings.js';
import {
  hydrateLibrary,
  LIBRARY_DOCUMENT_FIELDS,
  memberForRow,
  physicalRecordKey,
  rowsForMember,
} from './library-documents.js';

const PHYSICAL_SOURCE_ID = '00000000-0000-4000-8000-000000000011';
const SOURCE_VERSION_ID = '00000000-0000-4000-8000-000000000012';
const MEMBERSHIP_ID = '00000000-0000-4000-8000-000000000013';
const PROVENANCE_RULE_ID = '00000000-0000-4000-8000-000000000015';

const row: EntityRow = {
  record_key: 'drive-file',
  fields: { path: 'drive-file', title: 'One answer.md', body: '# One answer' },
  provenance: {
    path: {
      field: 'path',
      provenance_rule_version_id: PROVENANCE_RULE_ID,
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
      provenance_rule_version_id: PROVENANCE_RULE_ID,
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
  registry_id: 'google-drive',
  name: 'Google Drive',
  kind: 'google-drive',
  source_ids: [PHYSICAL_SOURCE_ID],
  editable: false,
  active: false,
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
      provenance_rule_version_id: PROVENANCE_RULE_ID,
    },
    rows: [row],
    sources: [
      {
        source_id: PHYSICAL_SOURCE_ID,
        source_version_id: SOURCE_VERSION_ID,
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
  memberships: [member],
};

const driveLibrary: LibrarySnapshot = {
  version: 7,
  sources: [],
  cloud_sources: [
    {
      id: 'drive-account',
      provider: 'google-drive',
      name: 'Google Drive',
      enabled: true,
      contract: {
        connector_version: 'google-drive/v1',
        capabilities: {
          read: true,
          write: false,
          search: true,
          watch: false,
          open_original: true,
          download: true,
        },
        mapping_version: 'mdreader.Document.cloud-mapping/v2',
        index_policy: 'read-through',
      },
    },
  ],
  connected_catdb: null,
  cleanup: { kind: 'complete' },
};

const localLibrary: LibrarySnapshot = {
  version: 7,
  sources: [
    {
      id: 'local-folder',
      name: 'Notes',
      path: '/notes',
      state: { kind: 'available' },
      index_state: 'present',
      active: true,
      restore_candidate: false,
    },
  ],
  cloud_sources: [],
  connected_catdb: null,
  cleanup: { kind: 'complete' },
};

describe('one hydrated Library answer', () => {
  it('uses one entity-query IPC and never issues provider-specific reads', async () => {
    const queryLibraryDocuments = vi.fn(async () => evidence);
    const backend = {
      listSources: vi.fn(async () => driveLibrary),
      prepareCloudCatalog: vi.fn(async () => ({
        workspace_id: '00000000-0000-4000-8000-000000000020',
        catalog_revision_id: '00000000-0000-4000-8000-000000000021',
        schema_version_id: '00000000-0000-4000-8000-000000000022',
      })),
      queryLibraryDocuments,
    };
    const result = await hydrateLibrary(backend, 'request-1');

    expect(result).toEqual({ evidence: [evidence], failures: [] });
    expect(queryLibraryDocuments).toHaveBeenCalledOnce();
    expect(queryLibraryDocuments).toHaveBeenCalledWith(
      '00000000-0000-4000-8000-000000000020',
      [],
      [
        'title',
        'path',
        'mediaType',
        'modifiedAt',
        'size',
        'checksum',
        'body',
      ],
      'request-1',
    );
  });

  it('requests only fields that registered local connectors can answer', async () => {
    const queryLibraryDocuments = vi.fn(async () => evidence);
    const backend = {
      listSources: vi.fn(async () => localLibrary),
      prepareCloudCatalog: vi.fn(async () => ({
        workspace_id: '00000000-0000-4000-8000-000000000020',
        catalog_revision_id: '00000000-0000-4000-8000-000000000021',
        schema_version_id: '00000000-0000-4000-8000-000000000022',
      })),
      queryLibraryDocuments,
    };

    await hydrateLibrary(backend, 'request-local');

    expect(queryLibraryDocuments).toHaveBeenCalledWith(
      '00000000-0000-4000-8000-000000000020',
      [],
      ['title', 'author', 'path', 'modifiedAt', 'size', 'body'],
      'request-local',
    );
  });

  it('does not publish a catalog or query when the Library has no enabled sources', async () => {
    const prepareCloudCatalog = vi.fn();
    const queryLibraryDocuments = vi.fn();
    const result = await hydrateLibrary(
      {
        listSources: vi.fn(async (): Promise<LibrarySnapshot> => ({
          version: 7,
          sources: [],
          cloud_sources: [],
          connected_catdb: null,
          cleanup: { kind: 'complete' },
        })),
        prepareCloudCatalog,
        queryLibraryDocuments,
      },
      'request-empty',
    );

    expect(result).toEqual({ evidence: [], failures: [] });
    expect(prepareCloudCatalog).not.toHaveBeenCalled();
    expect(queryLibraryDocuments).not.toHaveBeenCalled();
  });

  it('derives membership from selected provenance and keeps remote rows read-only', () => {
    expect(memberForRow(evidence, row)).toEqual(member);
    expect(rowsForMember(evidence, member)).toEqual([row]);
    expect(physicalRecordKey(row)).toBe('drive-file');
    expect(member.editable).toBe(false);
  });

  it('hydrates an authenticated Connected CatDB membership into the same Library result', async () => {
    const connectedEvidence: LibraryEntityQueryResponse_Serialize = {
      ...evidence,
      memberships: [
        {
          ...member,
          registry_id: 'connected-catdb',
          name: 'Research CatDB',
          kind: 'connected-catdb',
        },
      ],
    };
    const queryConnectedCatdbDocuments = vi.fn(async () => connectedEvidence);
    const result = await hydrateLibrary(
      {
        listSources: vi.fn(async (): Promise<LibrarySnapshot> => ({
          ...driveLibrary,
          connected_catdb: {
            connection_id: 'connected-catdb',
            label: 'Research CatDB',
            workspace_id: '00000000-0000-4000-8000-000000000030',
            catalog_revision_id: '00000000-0000-4000-8000-000000000031',
            selected_source_ids: [PHYSICAL_SOURCE_ID],
            contract: {
              connector_version: 'connected-catdb/v1',
              capabilities: {
                read: true,
                write: false,
                search: true,
                watch: false,
                open_original: false,
                download: false,
              },
              mapping_version: '00000000-0000-4000-8000-000000000031',
              index_policy: 'read-through',
            },
          },
        })),
        prepareCloudCatalog: vi.fn(async () => ({
          workspace_id: '00000000-0000-4000-8000-000000000020',
          catalog_revision_id: '00000000-0000-4000-8000-000000000021',
          schema_version_id: '00000000-0000-4000-8000-000000000022',
        })),
        queryLibraryDocuments: vi.fn(async () => evidence),
        queryConnectedCatdbDocuments,
      },
      'request-connected',
    );

    expect(result.evidence).toEqual([evidence, connectedEvidence]);
    expect(result.failures).toEqual([]);
    expect(queryConnectedCatdbDocuments).toHaveBeenCalledWith(
      'connected-catdb',
      [],
      [...LIBRARY_DOCUMENT_FIELDS],
      'request-connected:connected-catdb',
    );
    expect(connectedEvidence.memberships[0]?.editable).toBe(false);
  });

  it('keeps a healthy Connected CatDB answer when the embedded authority fails', async () => {
    const connectedEvidence: LibraryEntityQueryResponse_Serialize = {
      ...evidence,
      memberships: [
        {
          ...member,
          registry_id: 'connected-catdb',
          name: 'Research CatDB',
          kind: 'connected-catdb',
        },
      ],
    };
    const result = await hydrateLibrary(
      {
        listSources: vi.fn(async (): Promise<LibrarySnapshot> => ({
          ...driveLibrary,
          connected_catdb: {
            connection_id: 'connected-catdb',
            label: 'Research CatDB',
            workspace_id: '00000000-0000-4000-8000-000000000030',
            catalog_revision_id: '00000000-0000-4000-8000-000000000031',
            selected_source_ids: [PHYSICAL_SOURCE_ID],
            contract: {
              connector_version: 'connected-catdb/v1',
              capabilities: {
                read: true,
                write: false,
                search: true,
                watch: false,
                open_original: false,
                download: false,
              },
              mapping_version: '00000000-0000-4000-8000-000000000031',
              index_policy: 'read-through',
            },
          },
        })),
        prepareCloudCatalog: vi.fn(async () => {
          throw new Error('embedded catalog unavailable');
        }),
        queryLibraryDocuments: vi.fn(async () => evidence),
        queryConnectedCatdbDocuments: vi.fn(async () => connectedEvidence),
      },
      'request-partial',
    );

    expect(result.evidence).toEqual([connectedEvidence]);
    expect(result.failures).toEqual([{ authority: 'embedded' }]);
  });
});
