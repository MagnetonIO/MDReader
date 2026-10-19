import { renderHook } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, expect, test, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import {
  ReaderBackendProvider, readerBackend, setReaderBackend, unwrapNative, useReaderBackend, type ReaderBackend,
} from './backend';
import { folderIndexingBackend } from './folder-indexing';
import { tauriBackend } from './tauri-backend';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
let restore: (() => void) | undefined;
afterEach(() => { restore?.(); restore = undefined; vi.resetAllMocks(); });

function fakeBackend(folderIndexing: Partial<ReaderBackend['folderIndexing']>): ReaderBackend {
  return { ...tauriBackend, folderIndexing: { ...tauriBackend.folderIndexing, ...folderIndexing } };
}

test('unwrapNative rejects with the native error payload unchanged, as a raw invoke did', async () => {
  const payload = { kind: 'busy' };
  await expect(unwrapNative(Promise.resolve({ status: 'error', error: payload }))).rejects.toBe(payload);
  await expect(unwrapNative(Promise.resolve({ status: 'ok', data: 7 }))).resolves.toBe(7);
});

test('the default Tauri backend reaches the same native command and arguments', async () => {
  vi.mocked(invoke).mockResolvedValue([]);
  await expect(folderIndexingBackend.setMonitoring('source-1', true)).resolves.toEqual([]);
  expect(invoke).toHaveBeenCalledExactlyOnceWith('set_folder_monitoring', { sourceId: 'source-1', enabled: true });
});

test('a swapped backend serves modules without any native call, and restores', async () => {
  const folderIndexingState = vi.fn(async () => ({ status: 'ok' as const, data: [] }));
  restore = setReaderBackend(fakeBackend({ folderIndexingState }));
  await expect(folderIndexingBackend.state()).resolves.toEqual([]);
  expect(folderIndexingState).toHaveBeenCalledOnce();
  expect(invoke).not.toHaveBeenCalled();
  restore(); restore = undefined;
  expect(readerBackend()).toBe(tauriBackend);
});

test('the hook prefers the mounted provider and falls back to the active backend', () => {
  const mounted = fakeBackend({});
  const wrapper = ({ children }: { children: ReactNode }) =>
    <ReaderBackendProvider backend={mounted}>{children}</ReaderBackendProvider>;
  expect(renderHook(() => useReaderBackend(), { wrapper }).result.current).toBe(mounted);
  expect(renderHook(() => useReaderBackend()).result.current).toBe(tauriBackend);
});
