import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import { createRef } from 'react';
import { parseMarkdown } from '@mdreader/markdown';
import { MarkdownReader, type MarkdownReaderHandle } from './markdown-reader';
import { PlatformProvider } from './platform-provider';

// Model virtual mounting only. Real parsed IDs and the reader's scroll/frame
// lifecycle remain under test; this is not a native layout acceptance claim.
vi.mock('@tanstack/react-virtual', async () => {
  const { useState } = await import('react');
  return { useVirtualizer: ({ count, getScrollElement }: { count: number; getScrollElement: () => HTMLElement | null }) => {
    const [first, setFirst] = useState(0);
    return {
      scrollToIndex: (index: number) => setFirst(index),
      scrollToOffset: (offset: number) => { const scroll = getScrollElement(); if (scroll) scroll.scrollTop = offset; },
      getTotalSize: () => count * 140,
      measureElement: () => {},
      getVirtualItems: () => Array.from({ length: Math.min(4, count - first) }, (_, offset) => ({
        index: first + offset, key: first + offset, start: (first + offset) * 140,
      })),
    };
  } };
});
afterEach(() => { cleanup(); vi.unstubAllGlobals(); });
function geometry(container: HTMLElement) {
  const scroll = container.querySelector<HTMLElement>('.mdreader')!;
  scroll.getBoundingClientRect = () => ({ top: 100 }) as DOMRect;
  Array.from(scroll.querySelectorAll<HTMLElement>('[id]')).forEach((element, index) => {
    element.getBoundingClientRect = () => ({ top: 100 + index * 90 - scroll.scrollTop }) as DOMRect;
  });
  return scroll;
}
test('footnote and back-reference clicks scroll final nested IDs within this reader', () => {
  const ref = createRef<MarkdownReaderHandle>();
  const doc = parseMarkdown('# Note\n\nA fact[^one].\n\n[^one]: Its definition.');
  const view = render(<MarkdownReader doc={doc} ref={ref} onNavigate={(event) => {
    if (event.kind === 'anchor') expect(ref.current!.scrollToFragment(event.slug)).toBe(true);
  }} />);
  const scroll = geometry(view.container);
  const footnote = view.container.querySelector<HTMLAnchorElement>('a[href="#user-content-fn-one"]')!;
  const backlink = view.container.querySelector<HTMLAnchorElement>('a[href="#user-content-fnref-one"]')!;
  expect(footnote).toBeTruthy(); expect(backlink).toBeTruthy();
  fireEvent.click(footnote);
  const definitionPosition = scroll.scrollTop;
  expect(definitionPosition).toBeGreaterThan(0);
  fireEvent.click(backlink);
  expect(scroll.scrollTop).toBeLessThan(definitionPosition);
  expect(ref.current!.scrollToFragment('missing')).toBe(false);
});
test('virtualized fragment mounts its exact distant block and a document switch cancels the queued scroll', () => {
  const frames = new Map<number, FrameRequestCallback>(); let sequence = 0;
  vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => { frames.set(++sequence, callback); return sequence; });
  vi.stubGlobal('cancelAnimationFrame', (id: number) => frames.delete(id));
  const ref = createRef<MarkdownReaderHandle>();
  const doc = parseMarkdown(Array.from({ length: 310 }, (_, index) => `## Section ${index}\n`).join('\n'));
  const view = render(<MarkdownReader ref={ref} doc={doc} />);
  expect(screen.queryByRole('heading', { name: /Section 309/ })).toBeNull();
  act(() => { expect(ref.current!.scrollToFragment('section-309')).toBe(true); });
  expect(screen.getByRole('heading', { name: /Section 309/ })).toBeTruthy();
  const scroll = geometry(view.container);
  const heading = screen.getByRole('heading', { name: /Section 309/ });
  heading.getBoundingClientRect = () => ({ top: 550 - scroll.scrollTop }) as DOMRect;
  act(() => { const pending = [...frames.values()]; frames.clear(); pending.forEach((callback) => callback(0)); });
  expect(scroll.scrollTop).toBe(450);
  act(() => { ref.current!.scrollToFragment('section-308'); });
  view.rerender(<MarkdownReader ref={ref} doc={parseMarkdown('# Replacement')} />);
  expect(frames.size).toBe(0);
  expect(ref.current!.scrollToFragment('section-308')).toBe(false);
});
test('table and reference links preserve URI ingress once, handle modified clicks, and dispatch external opens', () => {
  const onNavigate = vi.fn(); const openExternal = vi.fn();
  const doc = parseMarkdown('## Unicode café\n\n| Link |\n| --- |\n| [Next](notes/Next%20note.md#section-two) |\n| [Here](#unicode-caf%C3%A9) |\n| [Web][site] |\n\n[site]: https://example.com/docs\n');
  render(<PlatformProvider platform={{ resolveAssetUrl: (url) => url, openExternal }}><MarkdownReader doc={doc} onNavigate={onNavigate} /></PlatformProvider>);
  fireEvent.click(screen.getByRole('link', { name: 'Next' }), { ctrlKey: true });
  expect(onNavigate).toHaveBeenLastCalledWith({ kind: 'relative', href: 'notes/Next%20note.md#section-two' });
  fireEvent.click(screen.getByRole('link', { name: 'Here' }));
  expect(onNavigate).toHaveBeenLastCalledWith({ kind: 'anchor', slug: 'unicode-café' });
  fireEvent(screen.getByRole('link', { name: 'Next' }), new MouseEvent('auxclick', { bubbles: true, cancelable: true, button: 1 }));
  expect(onNavigate).toHaveBeenCalledTimes(3);
  fireEvent.click(screen.getByRole('link', { name: 'Web' }));
  expect(openExternal).toHaveBeenCalledWith('https://example.com/docs');
});

test('virtual line/block/fragment placement shares measured column coordinates and settles before reporting anchors', () => {
  const frames = new Map<number, FrameRequestCallback>(); let sequence = 0;
  vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => { frames.set(++sequence, callback); return sequence; });
  vi.stubGlobal('cancelAnimationFrame', (id: number) => frames.delete(id));
  const ref = createRef<MarkdownReaderHandle>(), anchors = vi.fn();
  const doc = parseMarkdown('---\ntitle: Public fixture\n---\n\n' + Array.from({ length: 310 }, (_, index) => `## Section ${index}\n`).join('\n'));
  const view = render(<MarkdownReader ref={ref} doc={doc} onScrollAnchorChange={anchors} />);
  const scroll = view.container.querySelector<HTMLElement>('.mdreader')!;
  scroll.getBoundingClientRect = () => ({ top: 100 }) as DOMRect;
  let prefix = 192; // actual 32px column padding +160px frontmatter
  const frame = () => act(() => { const pending = [...frames.values()]; frames.clear(); pending.forEach(callback => callback(0)); });
  const placeGeometry = () => {
    for (const block of scroll.querySelectorAll<HTMLElement>('[data-block-index]')) {
      const index = Number(block.getAttribute('data-block-index'));
      block.getBoundingClientRect = () => ({ top: 100 + prefix + index * 140 - scroll.scrollTop }) as DOMRect;
      for (const heading of block.querySelectorAll<HTMLElement>('[id]')) heading.getBoundingClientRect = block.getBoundingClientRect;
    }
  };
  const settled = vi.fn();
  act(() => ref.current!.scrollToLine(doc.outline[309]!.lineStart, settled)); placeGeometry();
  fireEvent.scroll(scroll); expect(anchors).not.toHaveBeenCalled();
  frame(); expect(scroll.scrollTop).toBe(309 * 140 + 192); expect(settled).not.toHaveBeenCalled();
  prefix += 35; frame(); frame(); frame();
  expect(settled).toHaveBeenCalledWith(true); expect(ref.current!.getScrollAnchor()?.block).toBe(309);
  fireEvent.scroll(scroll); frame(); expect(anchors).toHaveBeenLastCalledWith({ block: 309, offset: 0 });
  act(() => ref.current!.scrollToBlock(308, 15)); placeGeometry(); frame(); frame(); frame();
  expect(ref.current!.getScrollAnchor()).toEqual({ block: 308, offset: 15 });
  act(() => ref.current!.scrollToFragment('section-309')); placeGeometry(); frame(); frame(); frame();
  expect(ref.current!.getScrollAnchor()).toEqual({ block: 309, offset: 0 });
});
