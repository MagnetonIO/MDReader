import type { SearchCoverage } from '@mdreader/ui';
import type { SearchCoverageDto } from '../bindings';

/** Copy owner observations without deriving coverage from hits or filenames.
 * Authority and index markers are independent opaque identities. */
export function normalizeSearchCoverage(value: SearchCoverageDto | null | undefined): SearchCoverage | null {
  if (!value) return null;
  return {
    scopeToken: value.scope_token,
    model: value.model,
    sources: value.sources.map(source => ({
      sourceId: source.source_id, kind: source.kind, authoritySnapshot: source.authority_snapshot,
      indexSnapshot: source.index_snapshot, observedAtMs: source.observed_at_ms,
      jobPhase: source.job_phase, jobGeneration: source.job_generation,
      indexedDocuments: source.indexed_documents, indexedChunks: source.indexed_chunks, embeddedChunks: source.embedded_chunks,
      eligibleDocuments: source.eligible_documents, admittedBodies: source.admitted_bodies,
      metadata: source.metadata ? { state: source.metadata.state, snapshot: source.metadata.snapshot, knownDocuments: source.metadata.known_documents } : null,
    })),
  };
}
