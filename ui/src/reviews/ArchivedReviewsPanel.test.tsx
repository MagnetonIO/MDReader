import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import { ArchivedReviewsPanel } from './ArchivedReviewsPanel';
import type { ArchivedReviewsBackend, ArchivedReviewRecord, ArchivedReviewComparison } from './adapter';
afterEach(cleanup);
const current = { sourceId: 'selected', path: 'renamed.md', revision: 'a'.repeat(64) };
const row: ArchivedReviewRecord = {
  recordId: 'b'.repeat(64), rowId: '9007199254740993', artifactIndex: 0, rowSha256: 'c'.repeat(64),
  state: 'rejected', reviewer: 'Recorded reviewer', recordedContentHash: 'blake3:recorded', reviewedContentHash: null,
  recordedHashComparison: 'unknown', graphMembership: 'unavailable', chronology: 'unavailable',
  schema: 'decision_overlay', extractor: 'mdreader.decision@1', rule: 'D1', qualified: 'decision_overlay.Decision',
  reading: '<script>archived assertion</script>', startLine: 2, endLine: 3, unavailableReason: null,
};
const compared: ArchivedReviewComparison = { review: row, current, observation: 'local_current_read', currentSpanHash: 'blake3:current', currentPassage: '<img src=x onerror=alert(1)>\nExact compared passage', recordedCurrent: 'matches', reviewedCurrent: 'unknown' };
function backend(overrides: Partial<ArchivedReviewsBackend> = {}): ArchivedReviewsBackend {
  return { select: vi.fn(async () => ({ handle: 'handle', exportId: 'export', manifestSha256: 'hash', ownerScope: 'profile' })),
    list: vi.fn(async () => ({ records: [row, { ...row, recordId: 'd'.repeat(64), state: 'unknown', reading: null, unavailableReason: 'unknown_review_state' }], nextOffset: null })),
    current: vi.fn(async () => current), compare: vi.fn(async () => compared), close: vi.fn(async () => {}), ...overrides };
}
async function select() {
  fireEvent.change(screen.getByLabelText('Export UUID'), { target: { value: '11111111-1111-4111-8111-111111111111' } });
  fireEvent.change(screen.getByLabelText('Manifest SHA-256'), { target: { value: 'a'.repeat(64) } });
  fireEvent.click(screen.getByRole('button', { name: 'Choose review stage…' }));
  await screen.findByText('rejected archival ruling');
}
test('explicit archive selection retains rejected/unknown records and compares only chosen exact current evidence', async () => {
  const api = backend(); render(<ArchivedReviewsPanel current={{ kind: 'local', path: '/selected/renamed.md', generation: 7 }} backend={api} onClose={() => {}} />);
  expect(api.select).not.toHaveBeenCalled(); expect(api.current).not.toHaveBeenCalled(); await select();
  expect(screen.getByText('unknown archival ruling')).toBeTruthy(); expect(screen.getByText(/unknown_review_state/)).toBeTruthy();
  expect(screen.queryByRole('button', { name: /Approve|Reject/ })).toBeNull(); expect(document.querySelector('script')).toBeNull();
  expect(screen.getByText('<script>archived assertion</script>', { selector: 'pre' })).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: 'Compare with open document' }));
  await screen.findByText('Recorded text versus current: matches');
  expect(screen.getByText('Reviewed text versus current: unknown')).toBeTruthy();
  expect(api.current).toHaveBeenCalledWith('/selected/renamed.md', 7);
  expect(api.compare).toHaveBeenCalledWith('handle', row.recordId, current, 7);
  expect(screen.getByText(/Exact compared passage/, { selector: 'pre' }).textContent).toBe(compared.currentPassage);
  expect(document.querySelector('img')).toBeNull();
});
test('a late comparison from a previous exact document cannot display a passage in the new context', async () => {
  let resolve!: (value: ArchivedReviewComparison) => void;
  const api = backend({ compare: vi.fn(() => new Promise<ArchivedReviewComparison>((done) => { resolve = done; })) });
  const view = render(<ArchivedReviewsPanel current={{ kind: 'exact', reference: current }} backend={api} onClose={() => {}} />); await select();
  fireEvent.click(screen.getByRole('button', { name: 'Compare with open document' }));
  await waitFor(() => expect(api.compare).toHaveBeenCalledOnce());
  view.rerender(<ArchivedReviewsPanel current={{ kind: 'exact', reference: { ...current, revision: 'f'.repeat(64) } }} backend={api} onClose={() => {}} />);
  await act(async () => { resolve(compared); });
  expect(screen.queryByLabelText('Current revision comparison')).toBeNull(); expect(api.current).not.toHaveBeenCalled();
});
test('unavailable comparison is shown explicitly while archival records remain visible and close releases selection', async () => {
  const api = backend({ compare: vi.fn(async () => { throw new Error('Selected revision changed'); }) });
  const view = render(<ArchivedReviewsPanel current={{ kind: 'exact', reference: current }} backend={api} onClose={() => {}} />); await select();
  fireEvent.click(screen.getByRole('button', { name: 'Compare with open document' }));
  expect((await screen.findByRole('alert')).textContent).toContain('Selected revision changed');
  expect(screen.getByText('rejected archival ruling')).toBeTruthy(); expect(screen.queryByLabelText('Current revision comparison')).toBeNull();
  view.unmount(); expect(api.close).toHaveBeenCalledWith('handle');
});
