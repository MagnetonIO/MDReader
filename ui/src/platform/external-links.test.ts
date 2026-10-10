import { afterEach, expect, test, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { openReaderExternal } from './external-links';
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
afterEach(() => { delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__; vi.restoreAllMocks(); vi.mocked(invoke).mockReset(); });
test('native external links use the OS command and never silently fall back to a WebView popup', async () => {
  Object.assign(window, { __TAURI_INTERNALS__: {} });
  const popup = vi.spyOn(window, 'open');
  vi.mocked(invoke).mockResolvedValue(undefined);
  await openReaderExternal('https://example.com/docs#section');
  expect(invoke).toHaveBeenCalledWith('reader_open_external', { url: 'https://example.com/docs#section' });
  vi.mocked(invoke).mockRejectedValue({ kind: 'unavailable' });
  await expect(openReaderExternal('mailto:reader@example.com')).rejects.toThrow('could not be opened');
  expect(popup).not.toHaveBeenCalled();
});
test.each(['javascript:alert(1)', 'file:///private/document.md', 'https://name:password@example.com', 'https://example.com/\n'])('refuses unsafe external target %s before platform access', async (url) => {
  const popup = vi.spyOn(window, 'open');
  await expect(openReaderExternal(url)).rejects.toThrow('invalid');
  expect(invoke).not.toHaveBeenCalled(); expect(popup).not.toHaveBeenCalled();
});
