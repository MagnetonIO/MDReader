import { afterEach, expect, test, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { readCatalogImage } from './local-image-backend';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
afterEach(() => vi.resetAllMocks());

test('the real generated image command refuses a reply from another exact receipt', async () => {
  const reference = { sourceId: 'a'.repeat(64), path: 'welcome.md', revision: 'b'.repeat(64), title: 'Welcome' };
  vi.mocked(invoke).mockResolvedValue({ operation: 'image', ...reference,
    revision: 'c'.repeat(64), target: 'images/pixel.png', mime: 'image/png', base64: 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+ip1sAAAAASUVORK5CYII=' });
  await expect(readCatalogImage(reference, 'images/pixel.png')).rejects.toThrow('This local image is unavailable');
  expect(invoke).toHaveBeenCalledExactlyOnceWith('reader_catalog', { request: {
    operation: 'image', sourceId: reference.sourceId, path: reference.path,
    revision: reference.revision, target: 'images/pixel.png',
  } });
});
