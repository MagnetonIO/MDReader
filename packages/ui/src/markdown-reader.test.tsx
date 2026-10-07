import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, test, vi } from 'vitest';
import { createRef } from 'react';
import { parseMarkdown } from '@mdreader/markdown';

import { MarkdownReader } from './markdown-reader.js';
import type { MarkdownReaderHandle } from './markdown-reader.js';
import { Outline } from './outline.js';

afterEach(cleanup);

/** Minimal IntersectionObserver double the tests drive by hand. */
interface FakeEntry {
  target: Element;
  isIntersecting: boolean;
  boundingClientRect: { top: number };
}
class MockIntersectionObserver {
  static instances: MockIntersectionObserver[] = [];
  observed = new Set<Element>();
  constructor(private cb: (entries: FakeEntry[]) => void) {
    MockIntersectionObserver.instances.push(this);
  }
  observe(el: Element): void {
    this.observed.add(el);
  }
  unobserve(el: Element): void {
    this.observed.delete(el);
  }
  disconnect(): void {
    this.observed.clear();
  }
  emit(entries: FakeEntry[]): void {
    this.cb(entries);
  }
}

describe('MarkdownReader', () => {
  test('renders headings and links from the parsed document', () => {
    const doc = parseMarkdown('# Hello World\n\nVisit [Example](https://example.com).\n');
    render(<MarkdownReader doc={doc} />);

    expect(screen.getByRole('heading', { level: 1 }).textContent).toContain('Hello World');
    const link = screen.getByRole('link', { name: 'Example' });
    expect(link.getAttribute('href')).toBe('https://example.com');
  });

  test('a wikilink click emits an onNavigate event with target and heading', () => {
    const onNavigate = vi.fn();
    const doc = parseMarkdown('See [[node-storage#Search|the store]] for details.\n');
    render(<MarkdownReader doc={doc} onNavigate={onNavigate} />);

    fireEvent.click(screen.getByText('the store'));

    expect(onNavigate).toHaveBeenCalledWith({
      kind: 'wikilink',
      target: 'node-storage',
      heading: 'Search',
    });
  });

  test('scrollToLine scrolls the containing source block without moving ancestor scroll containers', () => {
    const doc = parseMarkdown('# One\n\nPara two.\n\n## Three\n\nPara four.\n');
    const ref = createRef<MarkdownReaderHandle>();
    const { container } = render(<div data-outer><header>Mobile toolbar</header><MarkdownReader doc={doc} ref={ref} /></div>);
    const outer = container.querySelector<HTMLElement>('[data-outer]')!;
    const scroll = container.querySelector<HTMLElement>('.mdreader')!;
    outer.scrollTop = 83;
    document.documentElement.scrollTop = 19;
    scroll.scrollTop = 40;
    const tops = [0, 120, 260, 500];
    scroll.getBoundingClientRect = () => ({top:160}) as DOMRect;
    Array.from(scroll.querySelectorAll<HTMLElement>('[data-block-index]')).forEach((el,index)=>{
      el.getBoundingClientRect = () => ({top:160+tops[index]!-scroll.scrollTop}) as DOMRect;
    });
    // Model scrollIntoView's ancestor movement, the regression observed on iOS.
    const original=Element.prototype.scrollIntoView;
    Element.prototype.scrollIntoView=vi.fn(()=>{outer.scrollTop=999;document.documentElement.scrollTop=999;});
    try {
      ref.current!.scrollToLine(5);
      expect(scroll.scrollTop).toBe(260);
      expect(outer.scrollTop).toBe(83);
      expect(document.documentElement.scrollTop).toBe(19);
    } finally {Element.prototype.scrollIntoView=original;document.documentElement.scrollTop=0;}
  });

  test('getScrollAnchor + scrollToBlock round-trip lands in the same block at the same offset', () => {
    // h1, p, h2, p, h3, p → six top-level blocks (indices 0..5).
    const doc = parseMarkdown('# One\n\nAlpha.\n\n## Two\n\nBeta.\n\n### Three\n\nGamma.\n');
    const ref = createRef<MarkdownReaderHandle>();
    const { container } = render(<MarkdownReader doc={doc} ref={ref} />);

    const scroll = container.querySelector<HTMLElement>('.mdreader')!;
    const blocks = Array.from(scroll.querySelectorAll<HTMLElement>('[data-block-index]'));
    expect(blocks).toHaveLength(6);

    // jsdom has no layout: fake one. Container top is pinned at 0; each block's
    // content top is fixed, and its viewport rect.top follows scrollTop.
    const tops = [0, 120, 260, 500, 760, 1040];
    const rect = (top: number) => ({ top }) as unknown as DOMRect;
    let scrollTop = 0;
    Object.defineProperty(scroll, 'scrollTop', {
      configurable: true,
      get: () => scrollTop,
      set: (value: number) => {
        scrollTop = value;
      },
    });
    scroll.getBoundingClientRect = () => rect(0);
    blocks.forEach((el, index) => {
      el.getBoundingClientRect = () => rect((tops[index] ?? 0) - scrollTop);
    });

    // Rest deep inside block 2's long section (offset 65px past its top of 260).
    scrollTop = 325;
    const anchor = ref.current!.getScrollAnchor()!;
    expect(anchor).toEqual({ block: 2, offset: 65 });

    // Restoring from a clean scroll lands exactly where we were.
    scrollTop = 0;
    ref.current!.scrollToBlock(anchor.block, anchor.offset);
    expect(scrollTop).toBe(325);
    expect(ref.current!.getScrollAnchor()).toEqual(anchor);
  });

  test('getScrollAnchor works for a headingless document', () => {
    // No headings — three plain paragraphs. The anchor is a block index, not a
    // heading line, so persistence still works.
    const doc = parseMarkdown('Alpha paragraph.\n\nBeta paragraph.\n\nGamma paragraph.\n');
    const ref = createRef<MarkdownReaderHandle>();
    const { container } = render(<MarkdownReader doc={doc} ref={ref} />);

    const scroll = container.querySelector<HTMLElement>('.mdreader')!;
    const blocks = Array.from(scroll.querySelectorAll<HTMLElement>('[data-block-index]'));
    expect(blocks).toHaveLength(3);

    const tops = [0, 90, 210];
    const rect = (top: number) => ({ top }) as unknown as DOMRect;
    let scrollTop = 0;
    Object.defineProperty(scroll, 'scrollTop', {
      configurable: true,
      get: () => scrollTop,
      set: (value: number) => {
        scrollTop = value;
      },
    });
    scroll.getBoundingClientRect = () => rect(0);
    blocks.forEach((el, index) => {
      el.getBoundingClientRect = () => rect((tops[index] ?? 0) - scrollTop);
    });

    scrollTop = 130; // inside the second paragraph (top 90), 40px past it.
    expect(ref.current!.getScrollAnchor()).toEqual({ block: 1, offset: 40 });
  });

  test('renders a frontmatter card when the document has frontmatter', () => {
    const doc = parseMarkdown('---\ntitle: Doc\n---\n\n# Body\n');
    render(<MarkdownReader doc={doc} />);
    expect(screen.getByRole('button', { name: /Frontmatter/ })).toBeTruthy();
  });

  test('scrollToLine falls back to the nearest heading (section), not the preceding block', () => {
    // h1@1, p@3, p@5, h2@7, p@9: line 6 belongs to the H1 section.
    const doc = parseMarkdown('# H1\n\npara a\n\npara b\n\n## H2\n\npara c\n');
    const ref = createRef<MarkdownReaderHandle>();
    const {container}=render(<MarkdownReader doc={doc} ref={ref} />);
    const scroll=container.querySelector<HTMLElement>('.mdreader')!;
    scroll.scrollTop=230;
    scroll.getBoundingClientRect=()=>({top:100}) as DOMRect;
    Array.from(scroll.querySelectorAll<HTMLElement>('[data-block-index]')).forEach((el,index)=>{
      el.getBoundingClientRect=()=>({top:100+index*150-scroll.scrollTop}) as DOMRect;
    });
    ref.current!.scrollToLine(6);
    expect(scroll.scrollTop).toBe(0);
  });
});

describe('MarkdownReader scroll-spy (heading registration)', () => {
  const original = globalThis.IntersectionObserver;
  afterEach(() => {
    globalThis.IntersectionObserver = original;
    MockIntersectionObserver.instances = [];
  });

  function installMock() {
    MockIntersectionObserver.instances = [];
    globalThis.IntersectionObserver = MockIntersectionObserver as unknown as typeof IntersectionObserver;
  }

  test('registers mounted headings and reports the top-most intersecting one', () => {
    installMock();
    const onActive = vi.fn();
    const doc = parseMarkdown('# Alpha\n\ntext\n\n## Beta\n\nmore\n');
    const { container } = render(<MarkdownReader doc={doc} onActiveHeadingChange={onActive} />);

    const io = MockIntersectionObserver.instances[0]!;
    // Heading ids are namespaced `user-content-*` (the single slug source of
    // truth shared by anchors, outline slugs, and scroll-spy registration).
    const alpha = container.querySelector('#user-content-alpha')!;
    const beta = container.querySelector('#user-content-beta')!;
    // Both registered headings are observed.
    expect(io.observed.has(alpha)).toBe(true);
    expect(io.observed.has(beta)).toBe(true);

    // Only Beta visible -> active is Beta.
    act(() => {
      io.emit([{ target: beta, isIntersecting: true, boundingClientRect: { top: 40 } }]);
    });
    expect(onActive).toHaveBeenLastCalledWith('user-content-beta');

    // Both visible -> the top-most (smallest top) wins.
    act(() => {
      io.emit([{ target: alpha, isIntersecting: true, boundingClientRect: { top: 5 } }]);
    });
    expect(onActive).toHaveBeenLastCalledWith('user-content-alpha');
  });

  test('an explicit activeSlug prop overrides scroll-spy', () => {
    installMock();
    const onActive = vi.fn();
    const doc = parseMarkdown('# Alpha\n\n## Beta\n');
    const { container } = render(
      <MarkdownReader doc={doc} activeSlug="alpha" onActiveHeadingChange={onActive} />,
    );

    const io = MockIntersectionObserver.instances[0]!;
    const beta = container.querySelector('#user-content-beta')!;
    act(() => {
      io.emit([{ target: beta, isIntersecting: true, boundingClientRect: { top: 10 } }]);
    });

    // The prop is authoritative, so the derived-change callback stays silent.
    expect(onActive).not.toHaveBeenCalled();
    // And Beta's heading is not emphasized as active (Alpha is).
    expect(container.querySelector('#user-content-beta')?.getAttribute('style')).not.toContain(
      'var(--color-accent)',
    );
  });
});

describe('heading anchor + outline share the namespaced slug', () => {
  test('the heading id, its anchor href, and the outline slug all match (one source of truth)', () => {
    const doc = parseMarkdown('# My Section\n\nBody.\n');
    render(<MarkdownReader doc={doc} />);

    const heading = screen.getByRole('heading', { level: 1 });
    const id = heading.getAttribute('id');
    // rehype-slug namespaced the heading id, so it is prefixed.
    expect(id).toBe('user-content-my-section');

    // The hover anchor rendered inside the heading points at that same id — an
    // in-document click on it would resolve, because the id is what the DOM has.
    const anchor = heading.querySelector('a')!;
    expect(anchor.getAttribute('href')).toBe(`#${id}`);

    // The parsed outline slug (what an <Outline> renders) equals the heading id,
    // so outline highlighting/navigation targets the real element.
    expect(doc.outline[0]?.slug).toBe(id);
  });

  test('the outline link href and active highlight use the namespaced slug', () => {
    const doc = parseMarkdown('# My Section\n');
    const slug = doc.outline[0]!.slug;
    render(<Outline items={doc.outline} activeSlug={slug} />);

    const link = screen.getByRole('link', { name: 'My Section' });
    expect(link.getAttribute('href')).toBe(`#${slug}`);
    // The active slug matches the (prefixed) outline slug, so it highlights.
    expect(link.getAttribute('aria-current')).toBe('location');
  });
});
