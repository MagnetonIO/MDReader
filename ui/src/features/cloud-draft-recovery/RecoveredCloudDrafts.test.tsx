import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import { RecoveredCloudDrafts } from './RecoveredCloudDrafts';
import type { RecoveredDraft, RecoveredDraftBody, RecoveryBackend } from './backend';
import { saveDraftCopy } from '../document-transfer/draft-copy';
import { useProviderEdits } from '../provider-editing/store';
import { useEditorStore } from '../../stores/editor';
vi.mock('../document-transfer/draft-copy', () => ({ saveDraftCopy: vi.fn() }));
const draft: RecoveredDraft = { draftId: '11111111-1111-4111-8111-111111111111', revision: 'a'.repeat(64), title: 'Public cloud note.md', sourceName: 'Previous cloud connection', state: 'unknown', byteLength: 19 };
const markdown = '\uFEFF# Public draft\r\n';
const body: RecoveredDraftBody = { draft, markdown };
function backend(): RecoveryBackend { return { list: vi.fn(async offset => ({ drafts: [draft], offset, nextOffset: null })), read: vi.fn(async () => body) }; }
async function expandRecovery() { fireEvent.click(await screen.findByText('Recover cloud drafts…')); }
afterEach(() => { cleanup(); vi.resetAllMocks(); });
test('retained uncertain save opens local read-only text and copy cancellation preserves snapshot without reviving editor', async () => {
  const api = backend(); const edits = useProviderEdits.getState().edits, entries = useEditorStore.getState().entries;
  vi.mocked(saveDraftCopy).mockResolvedValue('cancelled');
  render(<RecoveredCloudDrafts backend={api} enabled />);
  await expandRecovery();
  const row = await screen.findByRole('button', { name: 'Public cloud note.md · Save snapshot' }); row.focus(); fireEvent.click(row);
  const text = await screen.findByRole('textbox', { name: 'Retained Markdown' });
  expect((text as HTMLTextAreaElement).readOnly).toBe(true);
  expect(screen.getByText(/Later unsaved changes may not be included/)).toBeTruthy();
  expect(screen.queryByRole('button', { name: /Save to provider|Retry save|Restore edit/ })).toBeNull();
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Save a copy…' })));
  expect(saveDraftCopy).toHaveBeenCalledExactlyOnceWith(markdown, draft.title);
  expect(screen.getByText('Copy not saved. Your retained draft is kept.')).toBeTruthy();
  expect(useProviderEdits.getState().edits).toBe(edits); expect(useEditorStore.getState().entries).toBe(entries);
  fireEvent.keyDown(text, { key: 'Escape' });
  expect(screen.queryByRole('dialog')).toBeNull();
  expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Public cloud note.md · Save snapshot' }));
});
test('close cancels late body publication and reopening performs another exact read', async () => {
  const api = backend(); let finish!: (value: RecoveredDraftBody) => void;
  vi.mocked(api.read).mockImplementation(() => new Promise(resolve => { finish = resolve; }));
  render(<RecoveredCloudDrafts backend={api} enabled />);
  await expandRecovery();
  const row = await screen.findByRole('button', { name: /Public cloud note.md/ }); fireEvent.click(row);
  fireEvent.click(screen.getByRole('button', { name: 'Close' }));
  await act(async () => finish(body)); expect(screen.queryByRole('textbox')).toBeNull();
  vi.mocked(api.read).mockResolvedValue(body); fireEvent.click(row);
  await screen.findByRole('textbox', { name: 'Retained Markdown' });
  expect(api.read).toHaveBeenCalledTimes(2); expect(api.read).toHaveBeenLastCalledWith(draft);
});
test('backend replacement fences stale old-library body', async () => {
  const api = backend(); let finish!: (value: RecoveredDraftBody) => void;
  vi.mocked(api.read).mockImplementation(() => new Promise(resolve => { finish = resolve; }));
  const view = render(<RecoveredCloudDrafts backend={api} enabled />);
  await expandRecovery();
  fireEvent.click(await screen.findByRole('button', { name: /Public cloud note.md/ }));
  const replacement = backend(); vi.mocked(replacement.list).mockResolvedValue({ drafts: [], offset: 0, nextOffset: null });
  view.rerender(<RecoveredCloudDrafts backend={replacement} enabled />);
  await act(async () => finish(body)); expect(screen.queryByRole('textbox')).toBeNull(); expect(screen.queryByRole('dialog')).toBeNull();
});
test('read refusal leaves metadata recoverable and offers no remote action', async () => {
  const api = backend(); vi.mocked(api.read).mockRejectedValue(new Error('stale'));
  render(<RecoveredCloudDrafts backend={api} enabled />);
  await expandRecovery();
  fireEvent.click(await screen.findByRole('button', { name: /Public cloud note.md/ }));
  await screen.findByRole('alert'); expect(screen.queryByRole('button', { name: 'Save a copy…' })).toBeNull();
  expect(screen.queryByRole('button', { name: /Fetch|Repair|Retry save/ })).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: 'Close' })); expect(screen.getByRole('button', { name: /Public cloud note.md/ })).toBeTruthy();
});
test('local listing pages remain bounded and loading never fetches draft bodies', async () => {
  const api = backend(); vi.mocked(api.list).mockResolvedValueOnce({ drafts: [draft], offset: 0, nextOffset: 32 }).mockResolvedValueOnce({ drafts: [{ ...draft, draftId: '22222222-2222-4222-8222-222222222222', title: 'Second.md' }], offset: 32, nextOffset: null });
  render(<RecoveredCloudDrafts backend={api} enabled />);
  await expandRecovery();
  fireEvent.click(await screen.findByRole('button', { name: 'More drafts' }));
  await screen.findByRole('button', { name: 'Second.md · Save snapshot' });
  expect(api.list).toHaveBeenNthCalledWith(2, 32); expect(api.read).not.toHaveBeenCalled();
});
test('copy is single-flight and failure preserves read-only body', async () => {
  const api = backend(); let fail!: (cause: Error) => void;
  vi.mocked(saveDraftCopy).mockImplementation(() => new Promise((_resolve, reject) => { fail = reject; }));
  render(<RecoveredCloudDrafts backend={api} enabled />);
  await expandRecovery();
  fireEvent.click(await screen.findByRole('button', { name: /Public cloud note.md/ }));
  const button = await screen.findByRole('button', { name: 'Save a copy…' }); fireEvent.click(button); fireEvent.click(button);
  expect(saveDraftCopy).toHaveBeenCalledOnce();
  await act(async () => fail(new Error('Copy unconfirmed.')));
  await waitFor(() => expect(screen.getByRole('alert').textContent).toBe('Copy unconfirmed.'));
  expect(screen.getByRole('textbox', { name: 'Retained Markdown' })).toBeTruthy();
});

test('historical snapshots stay collapsed until explicit recovery expansion, without automatic body reads', async () => {
  const api = backend();
  vi.mocked(api.list).mockResolvedValue({ drafts: [draft, { ...draft, draftId: '22222222-2222-4222-8222-222222222222', state: 'published' }], offset: 0, nextOffset: null });
  const view = render(<RecoveredCloudDrafts backend={api} enabled />);
  await waitFor(() => expect(view.container.querySelectorAll('details button')).toHaveLength(2));
  const disclosure = view.container.querySelector('details')!;
  expect(disclosure.open).toBe(false);
  expect(api.read).not.toHaveBeenCalled();
  await expandRecovery();
  expect(disclosure.open).toBe(true);
  expect(screen.getAllByRole('button', { name: 'Public cloud note.md · Save snapshot' })).toHaveLength(2);
  fireEvent.click(screen.getByText('Recover cloud drafts…'));
  expect(disclosure.open).toBe(false);
  expect(api.read).not.toHaveBeenCalled();
});
