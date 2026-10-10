import { afterEach, expect, test, vi } from 'vitest';
import { loadMetadata } from './metadata-loading';
import type { CatalogPage } from './backend';

const page: CatalogPage = { operation: 'page', source: { catalogId: 'remote:fixture', name: 'Public notes', kind: 'google_drive', enabled: true, coverage: 'partial_selected_metadata', observedAtMs: 1, failure: null, librarySourceId: null }, documents: [], nextCursor: null };
function callbacks() {
  return { page: vi.fn(async () => page), current: () => true, stopped: () => false,
    progress: vi.fn(), observed: vi.fn(), publish: vi.fn(), retire: vi.fn() };
}
afterEach(() => vi.restoreAllMocks());

test('continues active metadata discovery past the old batch and time limits until completion', async () => {
  let batch = 0;
  vi.spyOn(Date, 'now').mockImplementation(() => batch * 10_000);
  const observer = callbacks();
  const refresh = vi.fn(async () => ({ operation: 'refresh' as const, pages: 1, metadataComplete: ++batch === 70 }));
  expect(await loadMetadata({ ...observer, refresh })).toBe('complete');
  expect(refresh).toHaveBeenCalledTimes(70);
  expect(observer.publish).toHaveBeenCalledTimes(70);
  expect(observer.observed).toHaveBeenLastCalledWith(true);
});

test('rejects repeated non-progress while allowing empty file pages that advanced native discovery', async () => {
  const observer = callbacks();
  const refresh = vi.fn().mockResolvedValueOnce({ operation: 'refresh', pages: 1, metadataComplete: false })
    .mockResolvedValue({ operation: 'refresh', pages: 0, metadataComplete: false });
  await expect(loadMetadata({ ...observer, refresh })).rejects.toThrow('stopped making progress');
  expect(refresh).toHaveBeenCalledTimes(4);
  expect(observer.publish).toHaveBeenCalledTimes(3);
});

test('retains verified native completion after retiring UI publication during an in-flight refresh', async () => {
  let current = true;
  const observer = callbacks();
  const refresh = vi.fn(async () => {
    current = false;
    return { operation: 'refresh' as const, pages: 1, metadataComplete: true };
  });
  expect(await loadMetadata({ ...observer, current: () => current, refresh })).toBe('retired');
  expect(observer.observed).toHaveBeenCalledExactlyOnceWith(true);
  expect(observer.page).not.toHaveBeenCalled();
  expect(observer.publish).not.toHaveBeenCalled();
});
