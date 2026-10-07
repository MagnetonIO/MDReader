import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { SearchBatch, SearchHitDto } from '../bindings';

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
    expect(outcome).toEqual({ hits: [], semanticUnavailable: 'model unavailable' });
  });
});
