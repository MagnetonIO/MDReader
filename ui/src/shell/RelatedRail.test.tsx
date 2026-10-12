import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import type { RelatedHit, RelatedOutcome, SearchAdapter } from '@mdreader/ui';
import { RelatedRail } from './RelatedRail';
afterEach(() => { cleanup(); vi.useRealTimers(); });
const hit: RelatedHit = { documentId: 'note', path: '/ws/next.md', filePath: 'next.md', name: 'Next note', score: 0.8,
  graphDirection: 'incoming', components: { graph: 1, concept: 0, sharedTags: 0.5, vector: null } };
const unavailable: RelatedOutcome['availability'] = { vector: 'no_anchor_vectors', anchorChunks: 1, anchorVectors: 0, legacyConcepts: 'not_migrated', legacyGraphEntities: 'not_migrated' };
function props(related: SearchAdapter['related']) { return { activeDoc: { path: '/ws/a.md', name: 'a.md' }, adapter: { search: vi.fn(), related } as SearchAdapter, open: true, onToggle: vi.fn(), onOpen: vi.fn(), workspaceKey: '/ws' }; }
test('backlinks and shared tags are useful without embeddings or technical diagnostics', async () => {
  const base = props(async () => ({ hits: [hit], availability: unavailable })); render(<RelatedRail {...base} />);
  const row = await screen.findByRole('button', { name: /Next note/ });
  expect(row.title).toContain('Links here'); expect(row.title).toContain('Shared tags');
  expect(screen.queryByText(/Vector matches|not migrated|0\.80/)).toBeNull();
  fireEvent.click(row); expect(base.onOpen).toHaveBeenCalledWith({ path: hit.path, name: hit.name });
});
test('empty results are simple and failures offer an actual retry', async () => {
  const related = vi.fn<SearchAdapter['related']>().mockRejectedValueOnce(new Error('private error')).mockResolvedValue({ hits: [], availability: unavailable });
  render(<RelatedRail {...props(related)} />);
  fireEvent.click(await screen.findByRole('button', { name: 'Retry' }));
  await screen.findByText('No related notes yet.'); expect(related).toHaveBeenCalledTimes(2);
  expect(screen.queryByText(/not migrated|Vector matches|private error/)).toBeNull();
});
test('canonical related uses exact source, revision and device identity; reopening preserves receipts', async () => {
  const base = props(vi.fn()); const reference = { sourceId: 'source-one', path: 'document.md', revision: 'revision', deviceDocumentId: 'device-id' };
  base.adapter.relatedReference = vi.fn(async () => [{ ...hit, sourceId: 'source-one', revision: 'revision', sourceKind: 'device' as const, deviceDocumentId: 'device-next', line: 37 }]);
  render(<RelatedRail {...base} canonicalReference={reference} />);
  const row = await screen.findByRole('button', { name: /Next note/ }); fireEvent.click(row);
  expect(base.adapter.relatedReference).toHaveBeenCalledWith(reference); expect(base.adapter.related).not.toHaveBeenCalled();
  expect(base.onOpen).toHaveBeenCalledWith(expect.objectContaining({ sourceId: 'source-one', revision: 'revision', sourceKind: 'device', deviceDocumentId: 'device-next', line: 37 }));
});
test('refresh after index/save refetches; stale source completion is dropped', async () => {
  let settle!: (hits: RelatedHit[]) => void;
  const related = vi.fn<SearchAdapter['related']>().mockImplementationOnce(() => new Promise(done => { settle = done; })).mockResolvedValue([]);
  const base = props(related); const view = render(<RelatedRail {...base} refreshKey={0} />);
  await waitFor(() => expect(related).toHaveBeenCalledOnce());
  view.rerender(<RelatedRail {...base} workspaceKey="/new-source" refreshKey={1} />);
  await act(async () => settle([hit]));
  await waitFor(() => expect(related).toHaveBeenCalledTimes(2)); expect(screen.queryByText('Next note')).toBeNull();
});
test('rapid selection is debounced; collapsed rail performs no work', () => {
  vi.useFakeTimers(); const related = vi.fn(async () => []); const base = props(related); const view = render(<RelatedRail {...base} />);
  view.rerender(<RelatedRail {...base} activeDoc={{ path: '/ws/b.md', name: 'b.md' }} />);
  act(() => vi.advanceTimersByTime(200)); expect(related).not.toHaveBeenCalled(); act(() => vi.advanceTimersByTime(100)); expect(related).toHaveBeenCalledExactlyOnceWith('/ws/b.md');
  view.rerender(<RelatedRail {...base} open={false} />); act(() => vi.advanceTimersByTime(1000)); expect(related).toHaveBeenCalledOnce(); expect(screen.getByTitle('Show related notes')).toBeTruthy();
});
