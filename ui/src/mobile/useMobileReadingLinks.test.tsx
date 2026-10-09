import { act, cleanup, renderHook } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import { commands, type DeviceDocument } from '../bindings';
import { nativeProvidersBackend } from '../platform/providers';
import { observeReadingLinks } from './document-transfer';
import { useMobileReadingLinks } from './useMobileReadingLinks';

vi.mock('./document-transfer', () => ({ observeReadingLinks: vi.fn(async () => () => {}) }));
afterEach(() => { cleanup(); vi.restoreAllMocks(); });
const device: DeviceDocument = { id: '11111111-1111-4111-8111-111111111111', source_id: 'a'.repeat(64), revision: 'b'.repeat(64), name: 'public.md', bytes: 20, pinned: false, line: 1, indexed: true };
const deviceReference = { documentId: device.id, sourceId: device.source_id, revision: device.revision, path: 'document.md', line: 3 };
const cloudReference = { sourceId: `provider:${'c'.repeat(64)}`, path: `records/${'d'.repeat(64)}.md`, revision: 'e'.repeat(64), line: 5 };
function fixture() {
  const navigation = { current: 0 };
  const handlers = { openDevice: vi.fn(async () => {}), openProvider: vi.fn(async () => {}), failed: vi.fn() };
  const hook = renderHook(() => useMobileReadingLinks(navigation, handlers));
  return { navigation, handlers, hook };
}

test('opens an exact imported receipt and preserves the requested line', async () => {
  const read = vi.spyOn(commands, 'mobileReaderRead').mockResolvedValue({ status: 'ok', data: { document: device, markdown: '# Public' } });
  const { handlers, hook } = fixture();
  await act(async () => hook.result.current(deviceReference));
  expect(read).toHaveBeenCalledExactlyOnceWith(device.id, device.revision);
  expect(handlers.openDevice).toHaveBeenCalledExactlyOnceWith(device, 3);
  expect(handlers.openProvider).not.toHaveBeenCalled();
});

test('OS and pasted links open verified cached cloud receipts without fresh fetch or authorization', async () => {
  const read = vi.spyOn(nativeProvidersBackend, 'readDocument').mockResolvedValue({ operation: 'read_document', ...cloudReference, markdown: '# Cached', lineStart: 1, lineEnd: 5 });
  const fetch = vi.spyOn(nativeProvidersBackend, 'fetchCurrent');
  const connect = vi.spyOn(nativeProvidersBackend, 'connect');
  const { handlers, hook } = fixture();
  await act(async () => hook.result.current(cloudReference));
  const receive = vi.mocked(observeReadingLinks).mock.calls.at(-1)![0];
  await act(async () => receive(cloudReference));
  expect(read).toHaveBeenCalledTimes(2);
  expect(handlers.openProvider).toHaveBeenCalledTimes(2);
  expect(handlers.openProvider).toHaveBeenLastCalledWith(expect.objectContaining(cloudReference));
  expect(handlers.openDevice).not.toHaveBeenCalled();
  expect(fetch).not.toHaveBeenCalled(); expect(connect).not.toHaveBeenCalled();
});

test('rejects native replies naming another device source or cloud revision', async () => {
  vi.spyOn(commands, 'mobileReaderRead').mockResolvedValue({ status: 'ok', data: { document: { ...device, source_id: 'f'.repeat(64) }, markdown: '# Wrong source' } });
  vi.spyOn(nativeProvidersBackend, 'readDocument').mockResolvedValue({ operation: 'read_document', ...cloudReference, revision: 'f'.repeat(64), markdown: '# New revision', lineStart: 1, lineEnd: 5 });
  const { handlers, hook } = fixture();
  await act(async () => {
    await expect(hook.result.current(deviceReference)).rejects.toThrow('exact device receipt');
    await expect(hook.result.current(cloudReference)).rejects.toThrow('No newer copy');
  });
  expect(handlers.openDevice).not.toHaveBeenCalled(); expect(handlers.openProvider).not.toHaveBeenCalled();
});

test('retired navigation and unmount fence slow exact reads', async () => {
  let finish!: (reply: Awaited<ReturnType<typeof commands.mobileReaderRead>>) => void;
  vi.spyOn(commands, 'mobileReaderRead').mockImplementation(() => new Promise(resolve => { finish = resolve; }));
  const { handlers, hook, navigation } = fixture();
  let pending!: Promise<void>;
  act(() => { pending = hook.result.current(deviceReference); });
  navigation.current++;
  await act(async () => { finish({ status: 'ok', data: { document: device, markdown: '# Public' } }); await pending; });
  expect(handlers.openDevice).not.toHaveBeenCalled();
  act(() => { pending = hook.result.current(deviceReference); });
  hook.unmount();
  await act(async () => { finish({ status: 'ok', data: { document: device, markdown: '# Public' } }); await pending; });
  expect(handlers.openDevice).not.toHaveBeenCalled();
});

test('a slow cloud failure cannot report an error after newer navigation retires it', async () => {
  let reject!: (error: Error) => void;
  vi.spyOn(nativeProvidersBackend, 'readDocument').mockImplementation(() => new Promise((_, fail) => { reject = fail; }));
  const { handlers, hook, navigation } = fixture();
  let pending!: Promise<void>;
  act(() => { pending = hook.result.current(cloudReference); });
  navigation.current++;
  await act(async () => { reject(new Error('Old source revoked')); await pending; });
  expect(handlers.openProvider).not.toHaveBeenCalled();
  expect(handlers.failed).not.toHaveBeenCalled();
});
