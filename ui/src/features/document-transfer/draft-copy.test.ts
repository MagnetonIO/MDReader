import { afterEach, expect, test, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { setTestPlatform } from '../../test/tauri-os';
import { saveDraftCopy } from './draft-copy';
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
afterEach(() => { delete (window as unknown as { __TAURI_INTERNALS__?: object }).__TAURI_INTERNALS__; vi.unstubAllGlobals(); vi.restoreAllMocks(); vi.resetAllMocks(); setTestPlatform('macos'); });
function native() { Object.defineProperty(window, '__TAURI_INTERNALS__', { value: {}, configurable: true }); }
test('native draft preservation passes snapshot bytes and filename only, without a source grant', async () => {
  native(); vi.mocked(invoke).mockResolvedValue({ state: 'exported' });
  const markdown = '\uFEFF# Unsaved\r\n';
  expect(await saveDraftCopy(markdown, 'Cloud note.md')).toBe('exported');
  expect(invoke).toHaveBeenCalledExactlyOnceWith('reader_export_draft', { snapshot: { markdown, filename: 'Cloud note.md' } });
});
test('native cancel is truthful and a refused existing destination gives useful recovery', async () => {
  native(); vi.mocked(invoke).mockResolvedValue({ state: 'cancelled' });
  expect(await saveDraftCopy('# Kept', 'note.md')).toBe('cancelled');
  vi.mocked(invoke).mockRejectedValue({ kind: 'destination_exists_or_unavailable' });
  await expect(saveDraftCopy('# Kept', 'note.md')).rejects.toThrow('Choose a new filename');
});
test('oversized UTF-8 snapshot is refused before creating a dialog', async () => {
  native(); await expect(saveDraftCopy('λ'.repeat(1024 * 1024 + 1), 'note.md')).rejects.toThrow('2 MiB');
  expect(invoke).not.toHaveBeenCalled();
});
test('web keeps its ordinary Blob download and does not claim native disk confirmation', async () => {
  const create = vi.fn(() => 'blob:synthetic-draft'); const revoke = vi.fn();
  vi.stubGlobal('URL', Object.assign(class extends URL {}, { createObjectURL: create, revokeObjectURL: revoke }));
  const click = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => {});
  expect(await saveDraftCopy('# Browser draft', 'note.md')).toBe('downloaded');
  expect(click).toHaveBeenCalledOnce(); expect(create).toHaveBeenCalledOnce(); expect(invoke).not.toHaveBeenCalled();
  await new Promise((resolve) => window.setTimeout(resolve, 0));
  expect(revoke).toHaveBeenCalledWith('blob:synthetic-draft');
});
