import { act, cleanup, renderHook, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { DeviceDocument } from '../bindings';
import { useIncomingMarkdown, type IncomingReply } from './useIncomingMarkdown';
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }));
const document: DeviceDocument = { id: '11111111-1111-4111-8111-111111111111', name: 'selected.md', source_id: 'a'.repeat(64), revision: 'b'.repeat(64), bytes: 12, pinned: false, line: 1, indexed: true };
const empty: IncomingReply = { document: null, remaining: 0, error: null, retained: false, open: false };
const imported: IncomingReply = { ...empty, document, open: true };
beforeEach(() => vi.mocked(invoke).mockReset().mockResolvedValue(empty));
afterEach(() => { cleanup(); vi.clearAllMocks(); });
function fixture(platform: 'ios' | 'android' = 'android') {
  const navigation = { current: 0 };
  const handlers = { imported: vi.fn(async () => {}), open: vi.fn(async () => {}) };
  const hook = renderHook(() => useIncomingMarkdown(platform, navigation, handlers));
  return { navigation, handlers, hook };
}
test('Open with refreshes the published library before opening its native receipt', async () => {
  vi.mocked(invoke).mockResolvedValueOnce(imported);
  const { handlers, hook } = fixture();
  await waitFor(() => expect(handlers.open).toHaveBeenCalledExactlyOnceWith(document));
  expect(listen).toHaveBeenCalledWith('mdreader://incoming-markdown', expect.any(Function));
  expect(invoke).toHaveBeenCalledExactlyOnceWith('mobile_reader_import_incoming');
  expect(handlers.imported.mock.invocationCallOrder[0]!).toBeLessThan(handlers.open.mock.invocationCallOrder[0]!);
  expect(hook.result.current.retained).toBe(false);
});
test('uncertain admission stays pending and waits for explicit retry', async () => {
  vi.mocked(invoke).mockResolvedValueOnce({ ...empty, remaining: 1, error: 'import_unavailable', retained: true });
  const { handlers, hook } = fixture();
  await waitFor(() => expect(hook.result.current.retained).toBe(true));
  expect(invoke).toHaveBeenCalledTimes(1); expect(handlers.open).not.toHaveBeenCalled();
  vi.mocked(invoke).mockResolvedValueOnce(imported);
  await act(async () => hook.result.current.retry());
  await waitFor(() => expect(handlers.open).toHaveBeenCalledExactlyOnceWith(document));
  expect(invoke).toHaveBeenCalledTimes(2);
});
test('late Open with publication cannot replace a newer navigation', async () => {
  let finish!: (value: IncomingReply) => void;
  vi.mocked(invoke).mockImplementationOnce(() => new Promise<IncomingReply>(resolve => { finish = resolve; }));
  const { handlers, navigation } = fixture();
  await waitFor(() => expect(invoke).toHaveBeenCalledTimes(1));
  navigation.current++;
  await act(async () => finish(imported));
  expect(handlers.imported).toHaveBeenCalledTimes(1); expect(handlers.open).not.toHaveBeenCalled();
});
test('iOS does not claim Android incoming queue support', async () => {
  fixture('ios'); await act(async () => {});
  expect(invoke).not.toHaveBeenCalled(); expect(listen).not.toHaveBeenCalled();
});
