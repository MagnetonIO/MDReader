import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import { MobileProviderReader } from './MobileProviderReader';
import { ProviderDocumentActionError, type ProviderDocumentActionsBackend, type ProviderReadingState } from './provider-document-actions';
import type { ProviderReaderReference } from '../platform/providers';
import type { CanonicalReaderBackend } from '../features/reader/contracts';
import { useSettingsStore } from '../stores/settings';
const reference: ProviderReaderReference = { sourceId: `provider:${'a'.repeat(64)}`, path: `records/${'b'.repeat(64)}.md`, revision: 'c'.repeat(64), title: 'Cloud note', line: 1 };
beforeEach(() => Object.defineProperty(window, '__TAURI_INTERNALS__', { configurable: true, value: {} }));
afterEach(() => { cleanup(); useSettingsStore.getState().reset(); Reflect.deleteProperty(window, '__TAURI_INTERNALS__'); });
function fixture() {
  let state: ProviderReadingState = { reference, pinned: true, line: 3, lineEnd: 3, generation: '4' };
  const reader: CanonicalReaderBackend = { readDocument: vi.fn(async exact => ({ operation: 'read_document', ...exact, markdown: '# Cloud note\n\nExact owned text.', lineStart: 1, lineEnd: 3 })) };
  const actions: ProviderDocumentActionsBackend = {
    read: vi.fn(async () => state),
    update: vi.fn(async (exact, expected, patch) => { state = { ...state, reference: exact, ...patch, generation: String(BigInt(expected) + 1n) }; return state; }),
    capabilities: vi.fn(async () => ({ export: true, share: true, readingLinks: false })),
    transfer: vi.fn(async (_exact, action) => action === 'share' ? 'presented' : 'exported'),
  };
  return { reader, actions };
}
test('cloud appearance uses the same controls without rereading, source writes or body remount', async () => {
  const { reader, actions } = fixture();
  const view = render(<MobileProviderReader reference={reference} onClose={() => {}} readerBackend={reader} actionsBackend={actions} />);
  await screen.findByRole('heading', { name: /Cloud note/ });
  const body = view.container.querySelector('.mdreader');
  fireEvent.click(screen.getByRole('button', { name: 'Reader options' }));
  fireEvent.click(screen.getByRole('tab', { name: 'Appearance' }));
  fireEvent.click(screen.getByRole('radio', { name: 'Sans serif' }));
  fireEvent.click(screen.getByRole('button', { name: 'Larger text' }));
  expect(view.container.querySelector('.mdreader')).toBe(body);
  const document = view.container.querySelector<HTMLElement>('.provider-reader-dialog__document')!;
  expect(document.style.getPropertyValue('--font-reading-serif')).toBe('var(--font-reading-sans)');
  expect(document.style.getPropertyValue('--text-0')).toBe('calc(1rem * 1.08)');
  expect(reader.readDocument).toHaveBeenCalledTimes(1);
  expect(actions.update).not.toHaveBeenCalled(); expect(actions.transfer).not.toHaveBeenCalled();
});
test('mobile provider controls resume native state and explicitly save/export/share only the displayed exact provider receipt', async () => {
  const { reader, actions } = fixture();
  render(<MobileProviderReader reference={reference} onClose={() => {}} readerBackend={reader} actionsBackend={actions} />);
  await screen.findByRole('heading', { name: /Cloud note/ });
  expect(reader.readDocument).toHaveBeenCalledWith(expect.objectContaining({ ...reference, line: 3 }));
  expect(actions.read).toHaveBeenCalledExactlyOnceWith(reference);
  expect(actions.update).not.toHaveBeenCalled(); expect(actions.transfer).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: 'Reader options' }));
  const unpin = await screen.findByRole('button', { name: 'Unpin document' });
  fireEvent.click(unpin);
  await waitFor(() => expect(actions.update).toHaveBeenCalledWith(reference, '4', { pinned: false }));
  await screen.findByText('Unpinned on this device.');
  const save = screen.getByRole('button', { name: 'Save reading position' });
  const observedLine = Number(save.title.match(/line (\d+)/)?.[1]);
  expect(observedLine).toBeGreaterThanOrEqual(1); expect(observedLine).toBeLessThanOrEqual(3);
  fireEvent.click(save);
  await waitFor(() => expect(actions.update).toHaveBeenCalledWith(reference, '5', { line: observedLine }));
  fireEvent.click(screen.getByRole('button', { name: 'Export Markdown' }));
  await screen.findByText('Markdown exported to your selected destination.');
  expect(actions.transfer).toHaveBeenCalledWith(expect.objectContaining({ sourceId: reference.sourceId, path: reference.path, revision: reference.revision }), 'export');
  fireEvent.click(screen.getByRole('button', { name: 'Share document copy' }));
  await screen.findByText(/Copy presented to the system share sheet/);
  expect(actions.transfer).toHaveBeenCalledWith(expect.objectContaining({ sourceId: reference.sourceId, path: reference.path, revision: reference.revision }), 'share');
  expect(reader.readDocument).toHaveBeenCalledTimes(1);
});
test('a revoked provider receipt retires visible body and all actions when native transfer revalidation fails', async () => {
  const { reader, actions } = fixture();
  let release!: (value: ProviderReadingState) => void;
  actions.update = vi.fn(() => new Promise<ProviderReadingState>(resolve => { release = resolve; }));
  actions.transfer = vi.fn(async () => { throw new ProviderDocumentActionError('stale'); });
  render(<MobileProviderReader reference={reference} onClose={() => {}} readerBackend={reader} actionsBackend={actions} />);
  await screen.findByRole('heading', { name: /Cloud note/ });
  fireEvent.click(screen.getByRole('button', { name: 'Reader options' }));
  fireEvent.click(await screen.findByRole('button', { name: 'Unpin document' }));
  await waitFor(() => expect(actions.update).toHaveBeenCalledWith(reference, '4', { pinned: false }));
  fireEvent.click(await screen.findByRole('button', { name: 'Export Markdown' }));
  await screen.findByText(/This cached revision or source is unavailable/);
  expect(screen.queryByText('Exact owned text.')).toBeNull();
  expect(screen.queryByRole('button', { name: 'Pin document' })).toBeNull();
  expect(screen.queryByRole('button', { name: 'Export Markdown' })).toBeNull();
  await act(async () => release({ reference, pinned: false, line: 3, lineEnd: 3, generation: '5' }));
  expect(screen.queryByText('Exact owned text.')).toBeNull();
  expect(screen.queryByRole('button', { name: 'Export Markdown' })).toBeNull();
});
test('late native state from a previous selection cannot mount its body or enable actions in the next receipt', async () => {
  const { reader, actions } = fixture();
  let release!: (value: ProviderReadingState) => void;
  const old = new Promise<ProviderReadingState>(resolve => { release = resolve; });
  const next = { ...reference, revision: 'd'.repeat(64), title: 'Next copy' };
  actions.read = vi.fn(async exact => exact.revision === reference.revision ? old : { reference: next, pinned: false, line: 1, lineEnd: 3, generation: '0' });
  const view = render(<MobileProviderReader reference={reference} onClose={() => {}} readerBackend={reader} actionsBackend={actions} />);
  await waitFor(() => expect(actions.read).toHaveBeenCalledWith(reference));
  view.rerender(<MobileProviderReader reference={next} onClose={() => {}} readerBackend={reader} actionsBackend={actions} />);
  await screen.findByRole('button', { name: 'Reader options' });
  fireEvent.click(screen.getByRole('button', { name: 'Reader options' }));
  await screen.findByRole('button', { name: 'Pin document' });
  await act(async () => release({ reference, pinned: true, line: 3, lineEnd: 3, generation: '9' }));
  expect(screen.queryByRole('button', { name: 'Unpin document' })).toBeNull();
  expect(reader.readDocument).not.toHaveBeenCalledWith(expect.objectContaining({ revision: reference.revision }));
  fireEvent.click(screen.getByRole('button', { name: 'Pin document' }));
  await waitFor(() => expect(actions.update).toHaveBeenCalledWith(next, '0', { pinned: true }));
});

test('an explicit incoming line-one link keeps line one even when the native saved place is later', async () => {
  const { reader, actions } = fixture();
  render(<MobileProviderReader reference={reference} resumeSavedPlace={false} onClose={() => {}} readerBackend={reader} actionsBackend={actions} />);
  await screen.findByRole('heading', { name: /Cloud note/ });
  expect(reader.readDocument).toHaveBeenCalledWith(expect.objectContaining({ ...reference, line: 1 }));
  expect(reader.readDocument).not.toHaveBeenCalledWith(expect.objectContaining({ line: 3 }));
  expect(actions.update).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: 'Reader options' }));
  expect(screen.getByRole('button', { name: 'Resume saved position' })).toBeTruthy();
});
