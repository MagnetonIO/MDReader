import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import { useEditorStore } from '../../stores/editor';
import { providerEditBackend } from './backend';
import { beginProviderEdit, useProviderEdits } from './store';
import { ProviderEditor } from './ProviderEditor';
import { ProviderEditsTray } from './ProviderEditsTray';
vi.mock('./backend', () => ({ providerEditBackend: { begin: vi.fn(), prepare: vi.fn(), dispatch: vi.fn(), inspect: vi.fn(), repair: vi.fn() } }));
vi.mock('../../shell/SourcePane', () => ({ SourcePane: ({ value }: { value: string }) => <textarea aria-label="Draft" readOnly value={value} /> }));
const reference = { sourceId: `provider:${'a'.repeat(64)}`, path: `records/${'b'.repeat(64)}.md`, revision: 'c'.repeat(64), title: 'Public note.md' };
const projection = { jobId: 'native-job', readerSourceId: reference.sourceId, path: reference.path, revision: 'd'.repeat(64) };
const editId = '11111111-1111-4111-8111-111111111111';
beforeEach(() => {
  vi.resetAllMocks(); useEditorStore.getState().reset(); useProviderEdits.setState({ edits: {} });
  vi.mocked(providerEditBackend.begin).mockResolvedValue({ operation: 'begin_edit_exact', writeSourceId: 'opaque-source', editId,
    markdown: '# Original', expiresAtMs: Date.now() + 60000, projection: { ...projection, revision: reference.revision } });
});
afterEach(() => { cleanup(); useEditorStore.getState().reset(); useProviderEdits.setState({ edits: {} }); });
async function mount() {
  const key = (await beginProviderEdit('reader:tab', reference, () => true))!;
  useEditorStore.getState().edit('reader:tab', key, '# Public changed buffer');
  const published = vi.fn();
  render(<><ProviderEditor editKey={key} onRead={vi.fn()} onPublished={published} /><ProviderEditsTray onOpen={vi.fn()} /></>);
  return { key, published };
}
test('actual in-flight save shows Saving, hides transient drafts/diagnostics, then shows Saved without another dispatch', async () => {
  let resolve!: (value: typeof projection) => void;
  vi.mocked(providerEditBackend.dispatch).mockReturnValue(new Promise((done) => { resolve = done; }));
  const { key, published } = await mount();
  expect(screen.getByRole('navigation', { name: 'Retained cloud edits' })).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: 'Save changes' }));
  await waitFor(() => expect(providerEditBackend.dispatch).toHaveBeenCalledOnce());
  expect(screen.getByRole('button', { name: 'Saving…' }).hasAttribute('disabled')).toBe(true);
  expect(screen.queryByRole('navigation', { name: 'Retained cloud edits' })).toBeNull();
  expect(screen.queryByText(editId, { exact: false })).toBeNull();
  expect(screen.queryByRole('button', { name: 'Check save status' })).toBeNull();
  expect(screen.queryByText('Waiting for the native provider operation…')).toBeNull();
  await act(async () => { resolve(projection); });
  expect(screen.getByText('Saved')).toBeTruthy();
  expect(published).toHaveBeenCalledOnce();
  expect(useEditorStore.getState().entry('reader:tab', key)).toMatchObject({ dirty: false, buffer: '# Public changed buffer' });
  expect(screen.queryByRole('navigation', { name: 'Retained cloud edits' })).toBeNull();
  expect(providerEditBackend.dispatch).toHaveBeenCalledOnce();
  act(() => useEditorStore.getState().edit('reader:tab', key, '# Newer unsaved buffer'));
  expect(screen.getByText('Unsaved changes')).toBeTruthy();
  expect(screen.queryByText('Saved', { exact: true })).toBeNull();
  expect(screen.queryByRole('button', { name: 'Check save status' })).toBeNull();
  expect(screen.getByRole('navigation', { name: 'Retained cloud edits' }).textContent).toContain('Unsaved changes');
});
test('unconfirmed save keeps the same draft actionable and status checking never dispatches again', async () => {
  vi.mocked(providerEditBackend.dispatch).mockRejectedValue(new Error('Provider confirmation unavailable'));
  vi.mocked(providerEditBackend.inspect).mockResolvedValue({ operation: 'inspect_save', operationId: editId, state: 'unknown', projection: null });
  const { key } = await mount();
  fireEvent.click(screen.getByRole('button', { name: 'Save changes' }));
  await screen.findByRole('button', { name: 'Check save status' });
  expect(screen.getByText('The save could not be confirmed. Your draft is kept.')).toBeTruthy();
  expect(screen.getByRole('navigation', { name: 'Retained cloud edits' }).textContent).toContain('Save needs attention');
  expect(screen.queryByText('Saved', { exact: true })).toBeNull();
  expect(screen.getByText('Save details').closest('details')?.open).toBe(false);
  fireEvent.click(screen.getByRole('button', { name: 'Check save status' }));
  await waitFor(() => expect(providerEditBackend.inspect).toHaveBeenCalledOnce());
  await waitFor(() => expect(screen.getByRole('button', { name: 'Check save status' })).toBeTruthy());
  expect(useProviderEdits.getState().edits[key]?.ticket.editId).toBe(editId);
  expect(useEditorStore.getState().entry('reader:tab', key)).toMatchObject({ dirty: true, buffer: '# Public changed buffer' });
  expect(providerEditBackend.dispatch).toHaveBeenCalledOnce();
});


test('preview and split keep the source instance and unsaved buffer; confirmed Save stays in the chosen editor view', async () => {
  vi.mocked(providerEditBackend.dispatch).mockResolvedValue(projection);
  const { key, published } = await mount();
  const source = screen.getByRole('textbox', { name: 'Draft' });
  fireEvent.click(screen.getByRole('button', { name: 'Preview' }));
  expect(source.closest('[hidden]')).not.toBeNull();
  await screen.findByRole('heading', { name: 'Public changed buffer' });
  fireEvent.click(screen.getByRole('button', { name: 'Split' }));
  expect(screen.getByRole('textbox', { name: 'Draft' })).toBe(source);
  expect(source.closest('[hidden]')).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: 'Save changes' }));
  await screen.findByText('Saved');
  expect(published).toHaveBeenCalledOnce();
  expect(screen.getByRole('button', { name: 'Split' }).getAttribute('aria-pressed')).toBe('true');
  expect(screen.getByRole('textbox', { name: 'Draft' })).toBe(source);
  expect(useEditorStore.getState().entry('reader:tab', key)?.mode).toBe('split');
});
