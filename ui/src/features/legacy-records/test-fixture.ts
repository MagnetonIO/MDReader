import type { EntityRow, LibraryEntityQueryResponse_Serialize, LibrarySourceExecution } from '../../bindings';
import type { HydratedLibrarySelection } from '../../shell/library-documents';
const physical = '00000000-0000-4000-8000-000000000011';
const sourceVersion = '00000000-0000-4000-8000-000000000012';
const mapping = '00000000-0000-4000-8000-000000000014';
const rule = '00000000-0000-4000-8000-000000000015';
const member: LibrarySourceExecution = {
  membership_id: '00000000-0000-4000-8000-000000000013', registry_id: 'historical-account',
  kind: 'google-drive', name: 'Historical account', source_ids: [physical], editable: true, active: false,
};
const row: EntityRow = {
  record_key: 'historical-key', fields: { title: 'Historical note', path: 'historical-key' },
  provenance: { path: { field: 'path', provenance_rule_version_id: rule, selected: 0, considered: [{
    source_version_id: sourceVersion, record_key: 'historical-key', mapping_version_id: mapping,
    physical_attribute: 'id', observed_at_ms: 1, state: { state: 'observed_value', value: 'historical-key' },
  }] } },
};
export const legacyEvidence: LibraryEntityQueryResponse_Serialize = {
  memberships: [member], window_truncated: false,
  answer: {
    query_id: '00000000-0000-4000-8000-000000000001',
    pins: {
      catalog_resolver_id: '00000000-0000-4000-8000-000000000002',
      catalog_revision_id: '00000000-0000-4000-8000-000000000003',
      schema_version_id: '00000000-0000-4000-8000-000000000004',
      physical_schema_version_ids: ['00000000-0000-4000-8000-000000000005'],
      mapping_version_ids: [mapping], source_version_ids: [sourceVersion],
      policy_version_ids: ['00000000-0000-4000-8000-000000000006'], provenance_rule_version_id: rule,
    },
    rows: [row], sources: [{ source_id: physical, source_version_id: sourceVersion, outcome: 'ran',
      freshness_ms: 0, scanned: 1, returned: 1, retry_after_ms: null }],
    completeness: { state: 'complete' }, next_cursor: null,
  },
};
export const legacySelection: HydratedLibrarySelection = {
  member, row, evidence: legacyEvidence,
  cloudDocument: { document_key: 'historical-document-key', title: 'Historical note',
    content: '# Historical body', edit_token: 'synthetic-retained-edit-receipt',
    cache_state: 'provider-fresh', modified_at_ms: 1 },
};
