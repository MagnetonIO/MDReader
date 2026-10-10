import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { SearchBatch, SearchHitDto, SearchCoverageDto } from '../bindings';

const transport = vi.hoisted(() => ({ search: vi.fn() }));
vi.mock('../bindings', () => ({ commands: { search: transport.search }, events: {} }));
vi.mock('@tauri-apps/api/core', () => ({
  Channel: class {
    onmessage?: (batch: SearchBatch) => void;
  },
}));
import { search } from './fs';

function hit(extra: Partial<SearchHitDto> = {}): SearchHitDto {
  return {
    chunk_id: 'chunk', document_id: 'doc', file_path: 'notes/plan.md',
    abs_path: '/selected/notes/plan.md', name: 'plan.md', heading_path: ['Plan'],
    start_line: 3, end_line: 5, score: 1, rrf_score: 1,
    boosts: { title: 0, path: 0, recency: 0, graph: 0 },
    lex_rank: null, sem_rank: 1, snippet: 'evidence', mtime: 42, explain: null,
    ...extra,
  };
}
beforeEach(() => { transport.search.mockReset(); });
describe('search intent and revision evidence over IPC', () => {
  it('sends Semantic explicitly and preserves scoped revision/line evidence in both final paths', async () => {
    const evidence = { ...hit(), source_id: 'selected-source', revision: 'a'.repeat(64) };
    transport.search.mockImplementation(async (_query, _opts, channel) => {
      channel.onmessage({ generation: 1, phase: 'final', hits: [evidence] });
      return { status: 'ok', data: { hits: [evidence], semantic_unavailable: null } };
    });
    const phases = vi.fn();
    const outcome = await search('paraphrase', { lexicalOnly: false, explain: true, intent: 'semantic' }, phases);
    expect(transport.search).toHaveBeenCalledWith('paraphrase', {
      limit: 20, lexical_only: false, explain: true, intent: 'semantic',
    }, expect.anything());
    const expected = expect.objectContaining({ sourceId: 'selected-source', revision: evidence.revision, startLine: 3, endLine: 5, semRank: 1, lexRank: null });
    expect(phases).toHaveBeenCalledWith('final', [expected]);
    expect(outcome.hits).toEqual([expected]);
  });
  it('omits intent for older callers and accepts older hit payloads without fabricated evidence', async () => {
    transport.search.mockResolvedValue({ status: 'ok', data: { hits: [hit()], semantic_unavailable: null } });
    const outcome = await search('term', { lexicalOnly: true, explain: false }, vi.fn());
    expect(transport.search).toHaveBeenCalledWith('term', { limit: 20, lexical_only: true, explain: false }, expect.anything());
    expect(outcome.hits[0]).not.toHaveProperty('sourceId');
    expect(outcome.hits[0]).not.toHaveProperty('revision');
  });
  it('does not stream lexical hits or return degraded lexical fallback for pure Semantic', async () => {
    transport.search.mockImplementation(async (_query, _opts, channel) => {
      channel.onmessage({ generation: 1, phase: 'lexical', hits: [hit({ lex_rank: 1, sem_rank: null })] });
      return { status: 'ok', data: { hits: [hit({ lex_rank: 1, sem_rank: null })], semantic_unavailable: 'model unavailable' } };
    });
    const phases = vi.fn();
    const outcome = await search('meaning', { lexicalOnly: false, explain: false, intent: 'semantic' }, phases);
    expect(phases).not.toHaveBeenCalled();
    expect(outcome).toEqual({ hits: [], semanticUnavailable: 'model unavailable', coverage: null });
  });
  it('sends only explicitly selected canonical provider sources in the same search request', async () => {
    const sourceId='provider:'+'a'.repeat(64);
    const evidence=hit({source_id:sourceId,revision:'b'.repeat(64),abs_path:'records/'+'c'.repeat(64)+'.md'});
    transport.search.mockResolvedValue({status:'ok',data:{hits:[evidence],semantic_unavailable:null}});
    const outcome=await search('quartz',{lexicalOnly:true,explain:false,intent:'exact',providerSources:[sourceId]},vi.fn());
    expect(transport.search).toHaveBeenCalledTimes(1);
    expect(transport.search).toHaveBeenCalledWith('quartz',{limit:20,lexical_only:true,explain:false,intent:'exact',provider_sources:[sourceId]},expect.anything());
    expect(outcome.hits[0]).toMatchObject({sourceId,revision:evidence.revision,path:evidence.abs_path});
  });
});

it('preserves native device identity in streamed and final hits without turning document.md into a filesystem path', async () => {
  const documentId = '00000000-0000-4000-8000-000000000001';
  const evidence = { ...hit(), document_id: documentId, device_document_id: documentId,
    source_kind: 'device', source_id: 'a'.repeat(64), revision: 'b'.repeat(64), abs_path: 'document.md' };
  transport.search.mockImplementation(async (_query, _opts, channel) => {
    channel.onmessage({ generation: 0, phase: 'final', hits: [evidence] });
    return { status: 'ok', data: { hits: [evidence], semantic_unavailable: null } };
  });
  const phases = vi.fn();
  const result = await search('synced', { lexicalOnly: true, explain: false }, phases);
  const expected = expect.objectContaining({ deviceDocumentId: documentId, sourceKind: 'device', sourceId: evidence.source_id,
    path: 'document.md', revision: evidence.revision, startLine: 3, endLine: 5 });
  expect(result.hits).toEqual([expected]); expect(phases).toHaveBeenCalledWith('final', [expected]);
});


it('publishes the authorized final content observation rather than inferring body coverage from streamed hits', async () => {
  const finalCoverage: SearchCoverageDto = { scope_token: 'selected-scope', model: 'ready', sources: [{
    source_id: 'selected-provider', kind: 'provider', authority_snapshot: 'grant-a', index_snapshot: 'index-b', observed_at_ms: 10,
    job_phase: 'error', job_generation: 4, indexed_documents: 2, indexed_chunks: 5, embedded_chunks: 1,
    eligible_documents: null, admitted_bodies: 3, metadata: { state: 'complete_selected', snapshot: 'metadata-c', known_documents: 40 },
  }] };
  transport.search.mockImplementation(async (_query, _opts, channel) => {
    channel.onmessage({ generation: 2, phase: 'final', hits: [hit()] });
    return { status: 'ok', data: { hits: [], semantic_unavailable: null, coverage: finalCoverage } };
  });
  const phases = vi.fn();
  const outcome = await search('body-only-public-term', { lexicalOnly: true, explain: false }, phases);
  expect(phases).toHaveBeenCalledWith('final', [expect.anything()]);
  expect(outcome.hits).toEqual([]);
  expect(outcome.coverage).toEqual({ scopeToken: 'selected-scope', model: 'ready', sources: [{
    sourceId: 'selected-provider', kind: 'provider', authoritySnapshot: 'grant-a', indexSnapshot: 'index-b', observedAtMs: 10,
    jobPhase: 'error', jobGeneration: 4, indexedDocuments: 2, indexedChunks: 5, embeddedChunks: 1,
    eligibleDocuments: null, admittedBodies: 3, metadata: { state: 'complete_selected', snapshot: 'metadata-c', knownDocuments: 40 },
  }] });
});
