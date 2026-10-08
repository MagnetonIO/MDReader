import { afterEach, expect, test, vi } from 'vitest';
import { LocalImages } from './local-images';
import type { LocalImageBytes } from './contracts';

const reference = { sourceId: 'a'.repeat(64), path: 'welcome.md', revision: 'b'.repeat(64), title: 'Welcome' };
const image: LocalImageBytes = { mime: 'image/png', base64: 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+ip1sAAAAASUVORK5CYII=' };
const NativeURL = URL;
afterEach(() => vi.unstubAllGlobals());

function urls() {
  let sequence = 0;
  const create = vi.fn(() => `blob:local-${++sequence}`);
  const revoke = vi.fn();
  vi.stubGlobal('URL', class extends NativeURL {
    static createObjectURL = create;
    static revokeObjectURL = revoke;
  });
  return { create, revoke };
}

test('visible local images bound native reads and decoded copies, releasing offscreen and disposed URLs', async () => {
  const { create, revoke } = urls();
  const completions: ((value: LocalImageBytes) => void)[] = [];
  const read = vi.fn(() => new Promise<LocalImageBytes>((resolve) => completions.push(resolve)));
  const images = new LocalImages(reference, read);
  for (const target of ['one.png', 'two.png', 'three.png']) images.retain(target, target);
  const first = images.load('one.png');
  const second = images.load('two.png');
  await expect(images.load('three.png')).rejects.toThrow('Two images are loading');
  expect(read).toHaveBeenCalledTimes(2);
  completions[0]!(image); completions[1]!(image);
  await Promise.all([first, second]);
  expect(images.url('one.png')).toBe('blob:local-1');
  expect(images.url('two.png')).toBe('blob:local-2');
  const third = images.load('three.png');
  completions[2]!(image); await third;
  expect(images.url('one.png')).toBeNull();
  expect(revoke).toHaveBeenCalledWith('blob:local-1');
  images.release('two.png', 'two.png');
  expect(images.url('two.png')).toBeNull();
  expect(revoke).toHaveBeenCalledWith('blob:local-2');
  images.dispose();
  expect(images.url('three.png')).toBeNull();
  expect(revoke).toHaveBeenCalledWith('blob:local-3');
  expect(create).toHaveBeenCalledTimes(3);
});

test('a native image reply arriving after document disposal cannot create or retain a Blob URL', async () => {
  const { create } = urls();
  let complete!: (value: LocalImageBytes) => void;
  const read = vi.fn(() => new Promise<LocalImageBytes>((resolve) => { complete = resolve; }));
  const images = new LocalImages(reference, read);
  images.retain('pixel.png', 'reader');
  const loading = images.load('pixel.png');
  images.dispose();
  complete(image); await loading;
  expect(create).not.toHaveBeenCalled();
  expect(images.url('pixel.png')).toBeNull();
  images.retain('pixel.png', 'late-render');
  await images.load('pixel.png');
  expect(read).toHaveBeenCalledOnce();
});
