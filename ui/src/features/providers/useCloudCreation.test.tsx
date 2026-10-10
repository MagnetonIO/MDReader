import { act, renderHook, waitFor } from '@testing-library/react';
import { expect, test, vi } from 'vitest';
import { NativeProviderError, type ProviderSource, type ProviderCreateResult, type ProviderCreateIntent } from '../../platform/providers';
import { useCloudCreation } from './useCloudCreation';
import { canCreateCloudDocument, cloudMarkdownFilename } from './cloud-create-model';
const source: ProviderSource = { sourceId: 'synthetic-source', readerSourceId: `provider:${'a'.repeat(64)}`, name: 'Public account', kind: 'google_drive', enabled: true,
  health: 'ready', selectedScope: { kind: 'drive_folder', folderId: 'root', recursive: true }, coverage: { kind: 'never_indexed' },
  capabilities: { read: true, list: true, changes: false, assets: false, guardedWrite: true } };
const projection = { jobId: 'public-job', readerSourceId: source.readerSourceId!, path: `records/${'b'.repeat(64)}.md`, revision: 'c'.repeat(64) };
const result = (operationId: string, state: ProviderCreateResult['state'], operation: ProviderCreateResult['operation'] = 'create_document'): ProviderCreateResult => ({ operation, operationId, state, projection: state === 'published' ? projection : null });

test('creation is limited to saved writable folders and a bounded Markdown basename', () => {
  expect(canCreateCloudDocument(source)).toBe(true);
  expect(canCreateCloudDocument({ ...source, selectedScope: { kind: 'scoped_root', recursive: false, path: '/Notes' } })).toBe(true);
  for (const patch of [{ enabled: false }, { health: 'reconnect_required' as const }, { capabilities: { ...source.capabilities, guardedWrite: false } },
    { selectedScope: { kind: 'exact_documents' as const, count: 1 } }, { selectedScope: { kind: 'drive_readonly_folder' as const, folderId: 'root', accountWide: true } }]) {
    expect(canCreateCloudDocument({ ...source, ...patch })).toBe(false);
  }
  expect(cloudMarkdownFilename(' Notes ')).toBe('Notes.md');
  expect(cloudMarkdownFilename('Notes.MARKDOWN')).toBe('Notes.MARKDOWN');
  for (const name of ['', '.md', '../note', 'folder/note.md', 'folder\\note', 'bad\nname', 'é'.repeat(120)]) expect(cloudMarkdownFilename(name)).toBeNull();
});

test('closing during dispatch retains the exact UUID and only status inspection can publish and open', async () => {
  let finish!: (value: ProviderCreateResult) => void;
  const backend = { createDocument: vi.fn((_intent: ProviderCreateIntent) => new Promise<ProviderCreateResult>(resolve => { finish = resolve; })), inspectCreate: vi.fn() };
  const opened = vi.fn();
  const view = renderHook(() => useCloudCreation(backend, 0, opened));
  act(() => view.result.current.open(source));
  act(() => view.result.current.create());
  const intent = backend.createDocument.mock.calls[0]![0];
  act(() => view.result.current.close());
  await act(async () => finish(result(intent.operationId, 'accepted')));
  expect(opened).not.toHaveBeenCalled();
  act(() => view.result.current.open(source));
  expect(view.result.current.pending).toBe(true);
  act(() => view.result.current.create());
  expect(backend.createDocument).toHaveBeenCalledTimes(1);
  backend.inspectCreate.mockResolvedValue(result(intent.operationId, 'published', 'inspect_create'));
  await act(async () => view.result.current.inspect());
  expect(backend.inspectCreate).toHaveBeenCalledWith(source.sourceId, intent.operationId);
  expect(opened).toHaveBeenCalledOnce();
  expect(opened).toHaveBeenCalledWith({ sourceId: projection.readerSourceId, path: projection.path, revision: projection.revision, title: 'Untitled.md', line: 1 });
});

test('unknown outcome adopts the native prior operation, absent allows explicit same-UUID retry, conflict uses a fresh UUID', async () => {
  const prior = 'a1111111-1111-4111-8111-111111111111';
  const backend = { createDocument: vi.fn().mockRejectedValueOnce(new NativeProviderError({ kind: 'unknown_write', operationId: prior })), inspectCreate: vi.fn() };
  const view = renderHook(() => useCloudCreation(backend, 0, vi.fn()));
  act(() => view.result.current.open(source));
  await act(async () => view.result.current.create());
  expect(view.result.current.pending).toBe(true);
  backend.inspectCreate.mockResolvedValueOnce(result(prior, 'absent', 'inspect_create'));
  await act(async () => view.result.current.inspect());
  expect(backend.inspectCreate).toHaveBeenCalledWith(source.sourceId, prior);
  expect(view.result.current.retry).toBe(true);
  backend.createDocument.mockImplementationOnce(intent => Promise.resolve(result(intent.operationId, 'conflict')));
  await act(async () => view.result.current.create());
  expect(backend.createDocument.mock.calls[1]![0].operationId).toBe(prior);
  act(() => view.result.current.setFilename('Different.md'));
  backend.createDocument.mockImplementationOnce(intent => Promise.resolve(result(intent.operationId, 'unknown')));
  await act(async () => view.result.current.create());
  expect(backend.createDocument.mock.calls[2]![0].operationId).not.toBe(prior);
});

test.each(['wrong-operation', 'wrong-id', 'wrong-binding', 'invalid-path'])('refuses malformed publication %s without enabling a resend', async problem => {
  const backend = { createDocument: vi.fn(async (intent: ProviderCreateIntent) => {
    const value = result(intent.operationId, 'published');
    if (problem === 'wrong-operation') value.operation = 'inspect_create';
    if (problem === 'wrong-id') value.operationId = 'different';
    if (problem === 'wrong-binding') value.projection = { ...projection, readerSourceId: `provider:${'d'.repeat(64)}` };
    if (problem === 'invalid-path') value.projection = { ...projection, path: '../private.md' };
    return value;
  }), inspectCreate: vi.fn() };
  const opened = vi.fn(), view = renderHook(() => useCloudCreation(backend, 0, opened));
  act(() => view.result.current.open(source));
  await act(async () => view.result.current.create());
  expect(view.result.current.pending).toBe(true);
  act(() => view.result.current.create());
  expect(backend.createDocument).toHaveBeenCalledOnce();
  expect(opened).not.toHaveBeenCalled();
});

test('a late publication after scope change is retained for inspection without stealing the current selection', async () => {
  let finish!: (value: ProviderCreateResult) => void;
  const backend = { createDocument: vi.fn((_intent: ProviderCreateIntent) => new Promise<ProviderCreateResult>(resolve => { finish = resolve; })), inspectCreate: vi.fn() };
  const opened = vi.fn(), view = renderHook(scope => useCloudCreation(backend, scope, opened), { initialProps: 0 });
  act(() => view.result.current.open(source));
  act(() => view.result.current.create());
  const id = backend.createDocument.mock.calls[0]![0].operationId;
  view.rerender(1);
  await act(async () => finish(result(id, 'published')));
  await waitFor(() => expect(view.result.current.busy).toBe(false));
  expect(view.result.current.pending).toBe(true);
  expect(opened).not.toHaveBeenCalled();
});

test('explicit create waits for a bounded discovery batch to finish and retains intent when closed meanwhile', async () => {
  let drained!: () => void;
  const beforeCreate = vi.fn(() => new Promise<void>(resolve => { drained = resolve; }));
  const backend = { createDocument: vi.fn(async (intent: ProviderCreateIntent) => result(intent.operationId, 'unknown')), inspectCreate: vi.fn() };
  const view = renderHook(() => useCloudCreation(backend, 0, vi.fn(), beforeCreate));
  act(() => view.result.current.open(source));
  act(() => view.result.current.create());
  expect(beforeCreate).toHaveBeenCalledOnce();
  expect(backend.createDocument).not.toHaveBeenCalled();
  act(() => view.result.current.close());
  await act(async () => drained());
  expect(backend.createDocument).toHaveBeenCalledOnce();
  act(() => view.result.current.open(source));
  expect(view.result.current.pending).toBe(true);
});

test('unmount while native creation is in flight never opens a stale reader selection', async () => {
  let finish!: (value: ProviderCreateResult) => void;
  const backend = { createDocument: vi.fn((_intent: ProviderCreateIntent) => new Promise<ProviderCreateResult>(resolve => { finish = resolve; })), inspectCreate: vi.fn() };
  const opened = vi.fn(), view = renderHook(() => useCloudCreation(backend, 0, opened));
  act(() => view.result.current.open(source));
  act(() => view.result.current.create());
  const id = backend.createDocument.mock.calls[0]![0].operationId;
  view.unmount();
  await act(async () => finish(result(id, 'published')));
  expect(opened).not.toHaveBeenCalled();
});

test.each(['Notes: today', 'note?.md', 'star*.md', 'a"b.md', '<note>.md', 'a|b.md', '.MD'])('rejects native-forbidden filename %s before dispatch', name => {
  expect(cloudMarkdownFilename(name)).toBeNull();
});

test('an upfront native invalid rejection keeps the filename editable without pretending uncertainty', async () => {
  const kind = 'invalid';
  const backend = { createDocument: vi.fn().mockRejectedValue(new NativeProviderError({ kind })), inspectCreate: vi.fn() };
  const view = renderHook(() => useCloudCreation(backend, 0, vi.fn()));
  act(() => view.result.current.open(source));
  await act(async () => view.result.current.create());
  expect(view.result.current.pending).toBe(false);
  expect(view.result.current.retry).toBe(true);
  act(() => view.result.current.setFilename('Corrected.md'));
  expect(view.result.current.filename).toBe('Corrected.md');
  expect(backend.inspectCreate).not.toHaveBeenCalled();
});

test.each(['scope', 'backend'])('retirement of %s while draining discovery prevents creation', async changed => {
  let drained!: () => void;
  const beforeCreate = () => new Promise<void>(resolve => { drained = resolve; });
  const backend = { createDocument: vi.fn(), inspectCreate: vi.fn() };
  const replacement = { createDocument: vi.fn(), inspectCreate: vi.fn() };
  const view = renderHook(props => useCloudCreation(props.backend, props.scope, vi.fn(), beforeCreate), { initialProps: { backend, scope: 0 } });
  act(() => view.result.current.open(source));
  act(() => view.result.current.create());
  view.rerender({ backend: changed === 'backend' ? replacement : backend, scope: changed === 'scope' ? 1 : 0 });
  await act(async () => drained());
  expect(backend.createDocument).not.toHaveBeenCalled();
  expect(replacement.createDocument).not.toHaveBeenCalled();
  expect(view.result.current.pending).toBe(false);
  expect(view.result.current.retry).toBe(true);
});


test.each(['unauthorized', 'reconnect_required', 'credential_unavailable', 'unsupported'])('a native %s error after dispatch retains its UUID and requires inspection', async kind => {
  const backend = { createDocument: vi.fn().mockRejectedValue(new NativeProviderError({ kind })), inspectCreate: vi.fn() };
  const view = renderHook(() => useCloudCreation(backend, 0, vi.fn()));
  act(() => view.result.current.open(source));
  await act(async () => view.result.current.create());
  const original = backend.createDocument.mock.calls[0]![0];
  expect(view.result.current.pending).toBe(true);
  expect(view.result.current.retry).toBe(false);
  act(() => view.result.current.close());
  act(() => view.result.current.open(source));
  act(() => view.result.current.create());
  expect(backend.createDocument).toHaveBeenCalledOnce();
  backend.inspectCreate.mockResolvedValue(result(original.operationId, 'accepted', 'inspect_create'));
  await act(async () => view.result.current.inspect());
  expect(backend.inspectCreate).toHaveBeenCalledWith(source.sourceId, original.operationId);
  expect(view.result.current.pending).toBe(true);
});
