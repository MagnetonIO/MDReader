/** Final-result observations supplied by the source owner. File discovery and
 * body admission are separate from text indexing; null census means unknown. */
export interface SearchCoverageSource {
  sourceId: string;
  kind: 'local' | 'capture' | 'device' | 'provider';
  authoritySnapshot: string;
  indexSnapshot: string | null;
  observedAtMs: number | null;
  jobPhase: 'idle' | 'queued' | 'indexing' | 'loading' | 'embedding' | 'done' | 'model_unavailable' | 'error';
  jobGeneration: number | null;
  indexedDocuments: number;
  indexedChunks: number;
  embeddedChunks: number;
  eligibleDocuments: number | null;
  admittedBodies: number | null;
  metadata: { state: 'never_observed' | 'partial' | 'complete_selected'; snapshot: string | null; knownDocuments: number } | null;
}
export interface SearchCoverage {
  scopeToken: string;
  model: 'disabled' | 'not_downloaded' | 'loading' | 'ready' | 'busy' | 'unavailable' | 'restart_required';
  sources: SearchCoverageSource[];
}

export function indexObservation(source: SearchCoverageSource): string {
  const retained = source.indexSnapshot !== null;
  switch (source.jobPhase) {
    case 'queued': return retained ? 'Refresh queued; showing retained index.' : 'Text indexing queued.';
    case 'indexing': return retained ? 'Updating text index; showing retained index.' : 'Building text index.';
    case 'error': return retained ? 'Refresh failed; showing retained index.' : 'Text indexing failed; no index snapshot.';
    case 'loading': return 'Text index retained; semantic model is loading.';
    case 'embedding': return 'Text index retained; embedding document sections.';
    case 'model_unavailable': return 'Text index retained; semantic model unavailable.';
    default: return !retained ? 'No index snapshot.' : source.indexedDocuments === 0 ? 'No current indexed content.' : 'Retained text index.';
  }
}
export function semanticModelLabel(model: SearchCoverage['model']): string {
  switch (model) {
    case 'ready': return 'Semantic model ready';
    case 'disabled': return 'Semantic search disabled';
    case 'not_downloaded': return 'Semantic model not downloaded';
    case 'loading': return 'Semantic model loading';
    case 'busy': return 'Semantic model busy';
    case 'restart_required': return 'Semantic model requires restart';
    case 'unavailable': return 'Semantic model unavailable';
  }
}


/** Refresh state is visible even when the detailed observation is collapsed. */
export function coverageRefreshLabel(coverage: SearchCoverage): string | null {
  const phases = new Set(coverage.sources.map(source => source.jobPhase));
  const labels = [phases.has('error') ? 'refresh failed' : null,
    phases.has('indexing') ? 'updating text index' : null,
    phases.has('queued') ? 'refresh queued' : null].filter(Boolean);
  return labels.length ? labels.join('; ') : null;
}
