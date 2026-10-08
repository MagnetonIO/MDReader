import { afterEach, expect, test, vi } from 'vitest';
import { LocalImages } from './local-images';
import type { CanonicalReaderBackend, LocalImageBytes } from './contracts';

const reference = { sourceId: 'a'.repeat(64), path: 'welcome.md', revision: 'b'.repeat(64), title: 'Welcome' };
const image: LocalImageBytes = { mime: 'image/png', width: 1, height: 1, base64: 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+ip1sAAAAASUVORK5CYII=' };
const NativeURL = URL;
afterEach(() => vi.unstubAllGlobals());

function urls() {
  let sequence = 0;
  const create = vi.fn(() => `blob:local-${++sequence}`);
  const revoke = vi.fn();
  vi.stubGlobal('URL', class extends NativeURL {
    static override createObjectURL = create;
    static override revokeObjectURL = revoke;
  });
  return { create, revoke };
}

test('three visible small images load automatically in FIFO order with two native reads and no visible eviction', async () => {
  const { create, revoke } = urls();
  const completions: ((value: LocalImageBytes) => void)[] = [];
  const read = vi.fn<NonNullable<CanonicalReaderBackend['readImage']>>(() => new Promise<LocalImageBytes>((resolve) => completions.push(resolve)));
  const images = new LocalImages(reference, read);
  for (const target of ['one.png', 'two.png', 'three.png']) images.retain(target, target);
  const first = images.load('one.png');
  const second = images.load('two.png');
  const third = images.load('three.png');
  expect(read).toHaveBeenCalledTimes(2);
  completions[0]!(image); await first;
  expect(read).toHaveBeenCalledTimes(3);
  expect(read.mock.calls.map((call) => call[1])).toEqual(['one.png', 'two.png', 'three.png']);
  completions[1]!(image); completions[2]!(image);
  await Promise.all([first, second]);
  await third;
  expect(images.url('one.png')).toBe('blob:local-1');
  expect(images.url('two.png')).toBe('blob:local-2');
  expect(images.url('three.png')).toBe('blob:local-3');
  expect(revoke).not.toHaveBeenCalled();
  images.release('two.png', 'two.png');
  expect(images.url('two.png')).toBeNull();
  expect(revoke).toHaveBeenCalledWith('blob:local-2');
  images.dispose();
  expect(images.url('one.png')).toBeNull();
  expect(images.url('three.png')).toBeNull();
  expect(revoke).toHaveBeenCalledWith('blob:local-1');
  expect(revoke).toHaveBeenCalledWith('blob:local-3');
  expect(create).toHaveBeenCalledTimes(3);
});

test('a native image reply arriving after document disposal cannot create or retain a Blob URL', async () => {
  const { create } = urls();
  let complete!: (value: LocalImageBytes) => void;
  const read = vi.fn<NonNullable<CanonicalReaderBackend['readImage']>>(() => new Promise<LocalImageBytes>((resolve) => { complete = resolve; }));
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

test('offscreen queued images and a disposed queue never issue native reads', async () => {
  urls();
  const completions: ((value: LocalImageBytes) => void)[] = [];
  const read = vi.fn<NonNullable<CanonicalReaderBackend['readImage']>>(() => new Promise<LocalImageBytes>((resolve) => completions.push(resolve)));
  const images = new LocalImages(reference, read);
  const targets = ['one.png', 'two.png', 'offscreen.png', 'active.png', 'disposed.png'];
  for (const target of targets) images.retain(target, target);
  const loading = targets.map((target) => images.load(target));
  images.release('offscreen.png', 'offscreen.png');
  completions[0]!(image); await loading[0];
  expect(read).toHaveBeenCalledTimes(3);
  images.dispose();
  completions[1]!(image); completions[2]!(image);
  await Promise.all(loading);
  expect(read.mock.calls.map((call) => call[1])).toEqual(['one.png', 'two.png', 'active.png']);
  expect(images.url('disposed.png')).toBeNull();
});

test('aggregate pixel limits preserve visible copies and recover capacity when an image leaves view', async () => {
  const { revoke } = urls();
  const read = vi.fn<NonNullable<CanonicalReaderBackend['readImage']>>(async () => ({ ...image, width: 2000, height: 2000 }));
  const images = new LocalImages(reference, read);
  const targets = ['one.png', 'two.png', 'three.png', 'four.png', 'five.png'];
  for (const target of targets) images.retain(target, target);
  for (const target of targets.slice(0, 4)) await images.load(target);
  await expect(images.load('five.png')).rejects.toThrow('Too many images are visible');
  expect(revoke).not.toHaveBeenCalled();
  for (const target of targets.slice(0, 4)) expect(images.url(target)).not.toBeNull();
  images.release('one.png', 'one.png');
  await images.load('five.png');
  expect(images.url('five.png')).not.toBeNull();
  expect(revoke).toHaveBeenCalledExactlyOnceWith('blob:local-1');
  images.dispose();
});
