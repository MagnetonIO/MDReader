import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import { SearchPalette } from './search-palette.js';
import type { SearchAdapter, SearchOutcome } from './model.js';
import type { SearchCoverage, SearchCoverageSource } from './coverage.js';
import { SearchCoveragePanel } from './coverage-panel.js';

beforeEach(() => {
  Element.prototype.scrollIntoView = vi.fn();
  globalThis.ResizeObserver = class { observe() {} unobserve() {} disconnect() {} } as unknown as typeof ResizeObserver;
});
afterEach(cleanup);
const source = (changes: Partial<SearchCoverageSource> = {}): SearchCoverageSource => ({
  sourceId: 'selected-public-source', kind: 'local', authoritySnapshot: 'grant-one', indexSnapshot: 'index-two',
  observedAtMs: 100, jobPhase: 'done', jobGeneration: 2, indexedDocuments: 4, indexedChunks: 9, embeddedChunks: 2,
  eligibleDocuments: null, admittedBodies: null, metadata: null, ...changes,
});
const coverage = (sources = [source()]): SearchCoverage => ({ scopeToken: 'selected-scope', model: 'ready', sources });
const adapter = (value: SearchCoverage): SearchAdapter => ({ search: vi.fn(async () => ({ hits: [], semanticUnavailable: null, coverage: value })), related: vi.fn(async () => []) });
const props = { files: [{ path: '/public/filename.md', name: 'filename.md' }], activeDoc: null, sourceName: 'Public scope',
  workspaceKey: 'scope-one', isMac: true, onOpenHit: vi.fn(), onOpenFile: vi.fn(), onClose: vi.fn(), debounceMs: 0, reducedMotion: true };

test('content-only query goes to the content adapter and filename mode has no body coverage', async () => {
  const transport = adapter(coverage());
  const view = render(<SearchPalette {...props} adapter={transport} />);
  fireEvent.change(screen.getByRole('combobox'), { target: { value: 'term-only-in-public-body' } });
  await waitFor(() => expect(screen.getByLabelText('Content index coverage')).toBeTruthy());
  expect(transport.search).toHaveBeenCalledWith('term-only-in-public-body', { lexicalOnly: false, explain: true, intent: 'best' }, expect.any(Function));
  expect(screen.getByText('Indexed document text · Public scope')).toBeTruthy();
  expect(screen.queryByRole('option')).toBeNull();
  view.rerender(<SearchPalette {...props} mode="file" adapter={transport} />);
  expect(screen.queryByLabelText('Content index coverage')).toBeNull();
  expect(screen.getByText('Filenames and folders · Public scope')).toBeTruthy();
});

test('running and failed refresh disclose the retained observation without implying current filesystem completeness', () => {
  const view = render(<SearchCoveragePanel coverage={coverage([source({ jobPhase: 'indexing' })])} />);
  expect(screen.getByText(/Updating text index; showing retained index/)).toBeTruthy();
  expect(screen.getByText('4 Markdown documents indexed · eligible total unknown · updating text index')).toBeTruthy();
  expect(view.container.textContent).not.toMatch(/all indexed|100%|up.to.date/i);
  view.rerender(<SearchCoveragePanel coverage={coverage([source({ jobPhase: 'error' })])} />);
  expect(screen.getByText(/Refresh failed; showing retained index/)).toBeTruthy();
  expect(screen.getByText('4 Markdown documents indexed · eligible total unknown · refresh failed')).toBeTruthy();
  expect(view.container.textContent).toContain('Observed');
});

test('provider metadata and admitted bodies are separate from searchable text', () => {
  const view = render(<SearchCoveragePanel coverage={coverage([source({ kind: 'provider', indexedDocuments: 0, indexedChunks: 0, embeddedChunks: 0,
    admittedBodies: 3, metadata: { state: 'complete_selected', knownDocuments: 40, snapshot: 'metadata-observation' } })])} />);
  expect(screen.getByText('0 Markdown documents indexed · eligible total unknown')).toBeTruthy();
  expect(screen.getByText('Content scope: Cloud source')).toBeTruthy();
  expect(screen.getByText(/Cloud filenames: 40 \(selected metadata complete\)/)).toBeTruthy();
  expect(view.container.textContent).toContain('Document bodies recorded: 3. Filenames are not searchable document text.');
  expect(screen.getByText(/No current indexed content/)).toBeTruthy();
  expect(view.container.textContent).not.toContain('40 Markdown documents indexed');
});

test('ready model reports partial embeddings independently of the text index and unknown eligible total', () => {
  const view = render(<SearchCoveragePanel coverage={coverage()} />);
  expect(screen.getByText(/9 indexed content sections. Semantic model ready; 2 of 9 sections embedded/)).toBeTruthy();
  expect(view.container.textContent).not.toMatch(/all indexed|complete semantic|100%/i);
  view.rerender(<SearchCoveragePanel coverage={coverage([source({ kind: 'device', eligibleDocuments: 1, indexedDocuments: 1 })])} />);
  expect(screen.getByText('1 of 1 eligible Markdown documents indexed')).toBeTruthy();
  view.rerender(<SearchCoveragePanel coverage={coverage([source({ kind: 'device', eligibleDocuments: 1, indexedDocuments: 1 }), source({ sourceId: 'other', kind: 'provider' })])} />);
  expect(screen.getByText('5 Markdown documents indexed · eligible total unknown')).toBeTruthy();
});

test('hit-only batches cannot publish coverage, and a selected-scope switch fences a late final observation', async () => {
  let finish: ((outcome: SearchOutcome) => void) | undefined;
  const pending: SearchAdapter = { search: vi.fn((_query, _options, phase) => { phase('final', []); return new Promise<SearchOutcome>(resolve => { finish = resolve; }); }), related: vi.fn(async () => []) };
  const view = render(<SearchPalette {...props} adapter={pending} />);
  fireEvent.change(screen.getByRole('combobox'), { target: { value: 'public-content' } });
  await waitFor(() => expect(pending.search).toHaveBeenCalled());
  expect(screen.queryByLabelText('Content index coverage')).toBeNull();
  view.rerender(<SearchPalette {...props} workspaceKey="scope-two" adapter={adapter(coverage([source({ indexedDocuments: 1 })]))} />);
  await waitFor(() => expect(screen.getByText('1 Markdown documents indexed · eligible total unknown')).toBeTruthy());
  await act(async () => finish!({ hits: [], semanticUnavailable: null, coverage: coverage([source({ indexedDocuments: 99 })]) }));
  expect(screen.queryByText('99 Markdown documents indexed · eligible total unknown')).toBeNull();
});


test('empty content results keep the coverage disclosure keyboard reachable inside the palette', async () => {
  render(<SearchPalette {...props} adapter={adapter(coverage())} />);
  fireEvent.change(screen.getByRole('combobox'), { target: { value: 'public-content' } });
  await waitFor(() => expect(screen.getByLabelText('Content index coverage')).toBeTruthy());
  const summary = screen.getByLabelText('Content index coverage').querySelector('summary')!;
  summary.focus();
  fireEvent.keyDown(summary, { key: 'Tab' });
  expect(document.activeElement).toBe(screen.getByRole('tab', { name: 'Best' }));
});
