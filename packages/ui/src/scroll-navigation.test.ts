import { afterEach, expect, test, vi } from 'vitest';
import { createScrollNavigation } from './scroll-navigation';

afterEach(() => { vi.useRealTimers(); vi.unstubAllGlobals(); });
function layout() {
  const frames = new Map<number, FrameRequestCallback>(); let id = 0;
  vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => { frames.set(++id, callback); return id; });
  vi.stubGlobal('cancelAnimationFrame', (frame: number) => frames.delete(frame));
  const scroll = document.createElement('div'), target = document.createElement('div');
  let position = 0, top = 1400 + 32 + 160; // spacer estimate + padding + frontmatter
  Object.defineProperties(scroll, {
    scrollTop: { get: () => position, set: (value: number) => { position = Math.max(0, Math.min(2400, value)); } },
    scrollHeight: { value: 3000 }, clientHeight: { value: 600 },
  });
  scroll.getBoundingClientRect = () => ({ top: 100 }) as DOMRect;
  target.getBoundingClientRect = () => ({ top: 100 + top - position }) as DOMRect;
  return { scroll, target, frames, moveTarget: (next: number) => { top = next; },
    frame() { const callbacks = [...frames.values()]; frames.clear(); callbacks.forEach(callback => callback(0)); } };
}
test('virtual source placement includes column padding/frontmatter and converges after dynamic measurement', () => {
  const value = layout(), settled = vi.fn();
  const navigation = createScrollNavigation(() => value.scroll);
  // Model the public virtualizer's outstanding index target. Absolute placement
  // must replace it, otherwise later reconciliation returns to spacer offset.
  let indexTarget: number | null = null;
  navigation.start({ virtualized: true, mount: () => { indexTarget = 1400; value.scroll.scrollTop = 1400; },
    element: () => value.target, place: offset => { indexTarget = null; value.scroll.scrollTop = offset; }, settled });
  expect(navigation.active).toBe(true);
  value.frame(); expect(value.scroll.scrollTop).toBe(1592); expect(indexTarget).toBeNull(); expect(settled).not.toHaveBeenCalled();
  value.moveTarget(1627); value.frame(); expect(value.scroll.scrollTop).toBe(1627);
  value.frame(); expect(settled).not.toHaveBeenCalled();
  value.frame(); expect(settled).toHaveBeenCalledWith(true); expect(navigation.active).toBe(false);
});
test('bottom clamping displays the nearest location without certifying an exact source restoration', () => {
  const value = layout(), settled = vi.fn(); value.moveTarget(2890);
  const navigation = createScrollNavigation(() => value.scroll);
  navigation.start({ virtualized: true, mount: () => {}, element: () => value.target, offset: 65, settled });
  value.frame(); value.frame(); value.frame();
  expect(value.scroll.scrollTop).toBe(2400); expect(settled).toHaveBeenCalledWith(false);
});
test('new selection/user cancellation retires outstanding mounting and cannot scroll its replacement', () => {
  const value = layout(), old = vi.fn(), stop = vi.fn();
  const navigation = createScrollNavigation(() => value.scroll);
  navigation.start({ virtualized: true, mount: () => {}, element: () => null, settled: old, stop });
  navigation.cancel(); const position = value.scroll.scrollTop; value.frame();
  expect(stop).toHaveBeenCalledTimes(1); expect(old).toHaveBeenCalledWith(false); expect(value.scroll.scrollTop).toBe(position);
  const next = vi.fn();
  navigation.start({ virtualized: false, mount: () => {}, element: () => value.target, settled: next });
  expect(next).toHaveBeenCalledWith(true); expect(old).toHaveBeenCalledTimes(1);
});
test('a missing or suspended target ends unsuccessfully within the frame/time bounds', () => {
  vi.useFakeTimers();
  const value = layout(), settled = vi.fn(), navigation = createScrollNavigation(() => value.scroll);
  navigation.start({ virtualized: true, mount: () => {}, element: () => null, settled });
  for (let index = 0; index < 24; index++) value.frame();
  expect(settled).toHaveBeenCalledExactlyOnceWith(false); expect(value.frames.size).toBe(0);
  const suspended = vi.fn();
  navigation.start({ virtualized: true, mount: () => {}, element: () => value.target, settled: suspended });
  vi.advanceTimersByTime(750);
  expect(suspended).toHaveBeenCalledExactlyOnceWith(false); expect(value.frames.size).toBe(0); expect(navigation.active).toBe(false);
});


test('a short document beginning retains natural padding; another clipped block cannot certify restoration', () => {
  const scroll = document.createElement('div'), target = document.createElement('div');
  Object.defineProperties(scroll, { clientHeight: { value: 600 }, scrollHeight: { value: 600 } });
  scroll.getBoundingClientRect = () => ({ top: 100 }) as DOMRect;
  target.getBoundingClientRect = () => ({ top: 132 }) as DOMRect;
  const navigation = createScrollNavigation(() => scroll), beginning = vi.fn(), later = vi.fn();
  navigation.start({ virtualized: false, mount: () => {}, element: () => target, beginning: true, settled: beginning });
  expect(beginning).toHaveBeenCalledWith(true); expect(scroll.scrollTop).toBe(0);
  navigation.start({ virtualized: false, mount: () => {}, element: () => target, settled: later });
  expect(later).toHaveBeenCalledWith(false); expect(scroll.scrollTop).toBe(0);
});
