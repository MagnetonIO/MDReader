'use client';

import {
  Fragment,
  useCallback,
  useEffect,
  useImperativeHandle,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from 'react';
import { jsx, jsxs } from 'react/jsx-runtime';
import { toJsxRuntime } from 'hast-util-to-jsx-runtime';
import type { Components } from 'hast-util-to-jsx-runtime';
import { useVirtualizer } from '@tanstack/react-virtual';
import { shouldVirtualize } from '@mdreader/markdown';
import type { ParsedDoc } from '@mdreader/markdown';
import type { Element, ElementContent, Root as HastRoot, RootContent } from 'hast';
import type { ReactNode, Ref } from 'react';

import { CodeBlock } from './components/code-block.js';
import { FrontmatterCard } from './components/frontmatter-card.js';
import { MdImage } from './components/md-image.js';
import { MdLink } from './components/md-link.js';
import { MermaidBlock } from './components/mermaid-block.js';
import { WikiLink } from './components/wiki-link.js';
import { makeHeading } from './components/heading.js';
import { useOptionalPlatform } from './platform-provider.js';
import { ReaderProvider } from './reader-context.js';
import type { NavigateEvent } from './reader-context.js';
import { fragmentTarget } from './fragment-navigation.js';
import { createScrollNavigation } from './scroll-navigation.js';
import type { ScrollSettlement } from './scroll-navigation.js';

/** A restorable scroll position: the top-most visible block plus the pixel
 * offset scrolled past that block's top. Works for headingless documents and
 * for offsets deep inside a single long block (neither of which a
 * heading/line anchor can express). */
export interface ScrollAnchor {
  /** Index of the top-most (fully or partly visible) block. */
  block: number;
  /** Pixels scrolled past that block's top edge. */
  offset: number;
}

/** Imperative handle exposing the scroll API. */
export interface MarkdownReaderHandle {
  /** Scroll to a decoded final document ID (including footnotes). False means
   * the fragment is invalid or absent; never scrolls an outer page. */
  scrollToFragment(fragment: string, settled?: ScrollSettlement): boolean;
  /** Scroll to the block whose source line range contains `line`. The optional
   * callback reports bounded live-layout alignment, false on cancellation or
   * failure. It does not imply a durable reading-position write. */
  scrollToLine(line: number, settled?: ScrollSettlement): void;
  /** The current scroll anchor (top-most block + pixel offset), or `null`
   * before the scroll surface has mounted. */
  getScrollAnchor(): ScrollAnchor | null;
  /** Restore a scroll anchor: place `block`'s top at the viewport top, then
   * scroll `offset` pixels further. Clamps an out-of-range block. */
  scrollToBlock(block: number, offset: number, settled?: ScrollSettlement): void;
  /** Cancel an in-flight jump after explicit user input. */
  cancelScrollNavigation(): void;
}

export interface MarkdownReaderProps {
  /** The parsed document to render. */
  doc: ParsedDoc;
  /** Extra/override components for the hast → JSX substitution. */
  components?: Partial<Components>;
  /** Called when an in-document link (wikilink, anchor, relative) is activated. */
  onNavigate?: (event: NavigateEvent) => void;
  /** Highlight blocks whose source line range intersects this range. */
  highlightRange?: { start: number; end: number };
  /**
   * Slug of the active heading (drives outline/heading emphasis). When omitted,
   * the reader derives it from scroll-spy over the registered headings.
   */
  activeSlug?: string;
  /**
   * Notified when scroll-spy determines a new active heading (derived from the
   * registered, mounted heading elements). Lets a host drive an `<Outline>`.
   */
  onActiveHeadingChange?: (slug: string | undefined) => void;
  /** Optional resolver telling wikilinks whether their target exists. */
  resolveWikiLink?: (target: string) => boolean;
  /** Notified (rAF-throttled) with the current scroll anchor as the reader is
   * scrolled, so a host can persist and later restore the reading position. */
  onScrollAnchorChange?: (anchor: ScrollAnchor) => void;
  ref?: Ref<MarkdownReaderHandle>;
}

/**
 * Read the current scroll anchor from the live layout of the scroll container:
 * the top-most block whose top sits at or above the container's top edge, plus
 * how far we have scrolled past it. Works identically for the plain and
 * virtualized layouts (both render `[data-block-index]` wrappers for every
 * on-screen block; the top-most visible one is always mounted).
 */
function readAnchor(container: HTMLElement | null): ScrollAnchor | null {
  if (!container) return null;
  const containerTop = container.getBoundingClientRect().top;
  let best: { block: number; rel: number } | null = null;
  for (const el of container.querySelectorAll<HTMLElement>('[data-block-index]')) {
    const block = Number(el.getAttribute('data-block-index'));
    if (Number.isNaN(block)) continue;
    const rel = el.getBoundingClientRect().top - containerTop;
    // The top-most visible block: the greatest `rel` that is still at/above the
    // top edge (a small +1px tolerance for sub-pixel rounding).
    if (rel <= 1 && (!best || rel > best.rel)) best = { block, rel };
  }
  if (!best) return { block: 0, offset: 0 };
  return { block: best.block, offset: Math.max(0, Math.round(-best.rel)) };
}

function isElement(node: RootContent): node is Element {
  return node.type === 'element';
}

const HEADING_TAG = /^h[1-6]$/;

/** Recursively collect the text content of a hast element. */
function hastText(node: ElementContent): string {
  if (node.type === 'text') return node.value;
  if (node.type === 'element') return node.children.map(hastText).join('');
  return '';
}

/** Extract a `language-x` class from a code element's className list. */
function codeLanguage(code: Element): string {
  const className = code.properties?.className;
  const classes = Array.isArray(className) ? className.map(String) : [];
  const language = classes.find((name) => name.startsWith('language-'));
  return language ? language.slice('language-'.length) : '';
}

interface DispatchProps {
  node?: Element;
  children?: ReactNode;
  className?: string;
  'data-line-start'?: string;
  'data-line-end'?: string;
}

/**
 * Substitutes `<pre>` blocks: a `language-mermaid` fence becomes a MermaidBlock,
 * any other code fence a Shiki CodeBlock, and a bare `<pre>` renders normally.
 */
function PreDispatch({ node, children, ...rest }: DispatchProps) {
  const code = node?.children.find(
    (child): child is Element => child.type === 'element' && child.tagName === 'code',
  );
  const start = rest['data-line-start'];
  const end = rest['data-line-end'];

  if (!code) {
    return (
      <pre data-line-start={start} data-line-end={end}>
        {children}
      </pre>
    );
  }

  const language = codeLanguage(code);
  const source = hastText(code).replace(/\n$/, '');

  if (language === 'mermaid') {
    return <MermaidBlock source={source} dataLineStart={start} dataLineEnd={end} />;
  }
  return <CodeBlock code={source} lang={language} dataLineStart={start} dataLineEnd={end} />;
}

const BASE_COMPONENTS = {
  h1: makeHeading(1),
  h2: makeHeading(2),
  h3: makeHeading(3),
  h4: makeHeading(4),
  h5: makeHeading(5),
  h6: makeHeading(6),
  a: MdLink,
  img: MdImage,
  pre: PreDispatch,
  'wiki-link': WikiLink,
} as unknown as Partial<Components>;

/**
 * The reading surface. Renders a {@link ParsedDoc}'s hast to React with the
 * reader's component substitutions, an optional frontmatter card, and — for
 * large documents ({@link shouldVirtualize}) — windowed rendering via
 * `@tanstack/react-virtual` with dynamic measurement. Exposes `scrollToLine`.
 */
export function MarkdownReader({
  doc,
  components,
  onNavigate,
  highlightRange,
  activeSlug,
  onActiveHeadingChange,
  resolveWikiLink,
  onScrollAnchorChange,
  ref,
}: MarkdownReaderProps) {
  const platform = useOptionalPlatform();
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const scrollRaf = useRef<number | null>(null);
  const navigationRef = useRef<ReturnType<typeof createScrollNavigation> | null>(null);
  if (!navigationRef.current) navigationRef.current = createScrollNavigation(() => scrollRef.current);
  const navigation = navigationRef.current;
  useLayoutEffect(() => () => navigation.cancel(), [doc, navigation]);

  const mergedComponents = useMemo(
    () => ({ ...BASE_COMPONENTS, ...components }) as Partial<Components>,
    [components],
  );

  // Top-level blocks and their source line ranges.
  const blocks = useMemo(
    () => doc.hast.children.filter(isElement),
    [doc],
  );

  const blockLines = useMemo(
    () =>
      blocks.map((block) => ({
        start: Number(block.properties?.dataLineStart ?? 0),
        end: Number(block.properties?.dataLineEnd ?? 0),
        isHeading: HEADING_TAG.test(block.tagName),
      })),
    [blocks],
  );

  const renderedBlocks = useMemo(
    () =>
      blocks.map((block) => {
        const root: HastRoot = { type: 'root', children: [block] };
        return toJsxRuntime(root, {
          Fragment,
          jsx,
          jsxs,
          passNode: true,
          passKeys: true,
          components: mergedComponents,
        });
      }),
    [blocks, mergedComponents],
  );

  const virtualize = shouldVirtualize(doc.stats);

  const virtualizer = useVirtualizer({
    count: blocks.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => 140,
    overscan: 8,
    enabled: virtualize,
  });

  // --- Scroll-spy: headings register their DOM element; an IntersectionObserver
  // over those registered elements drives the derived active slug. Falls inert
  // (prop-driven only) where IntersectionObserver is unavailable (SSR/tests).
  const headingRegistry = useRef<Map<string, HTMLElement>>(new Map());
  const intersecting = useRef<Map<string, number>>(new Map());
  const observerRef = useRef<IntersectionObserver | null>(null);
  const [spyActiveSlug, setSpyActiveSlug] = useState<string | undefined>(undefined);

  const recomputeActive = useCallback(() => {
    // The active heading is the top-most one currently intersecting the viewport.
    let best: { slug: string; top: number } | undefined;
    for (const [slug] of headingRegistry.current) {
      const top = intersecting.current.get(slug);
      if (top === undefined) continue;
      if (!best || top < best.top) best = { slug, top };
    }
    if (best) setSpyActiveSlug(best.slug);
  }, []);

  useEffect(() => {
    if (typeof IntersectionObserver === 'undefined') return;
    const observer = new IntersectionObserver((entries) => {
      for (const entry of entries) {
        const slug = (entry.target as HTMLElement).id;
        if (!slug) continue;
        if (entry.isIntersecting) intersecting.current.set(slug, entry.boundingClientRect.top);
        else intersecting.current.delete(slug);
      }
      recomputeActive();
    });
    observerRef.current = observer;
    // Observe any headings that registered before the observer existed.
    for (const element of headingRegistry.current.values()) observer.observe(element);
    return () => {
      observer.disconnect();
      observerRef.current = null;
    };
  }, [recomputeActive]);

  const registerHeading = useCallback((slug: string, element: HTMLElement) => {
    headingRegistry.current.set(slug, element);
    observerRef.current?.observe(element);
    return () => {
      headingRegistry.current.delete(slug);
      intersecting.current.delete(slug);
      observerRef.current?.unobserve(element);
    };
  }, []);

  // Explicit prop wins; otherwise use the scroll-spy result.
  const effectiveActiveSlug = activeSlug ?? spyActiveSlug;

  useEffect(() => {
    if (activeSlug === undefined) onActiveHeadingChange?.(spyActiveSlug);
  }, [spyActiveSlug, activeSlug, onActiveHeadingChange]);

  function findBlockIndex(line: number): number {
    // Provenance-first: the block whose true source line range contains `line`.
    const contained = blockLines.findIndex((b) => line >= b.start && line <= b.end);
    if (contained >= 0) return contained;

    // Documented fallback: the nearest heading — i.e. the section the line falls
    // in — rather than silently snapping to the immediately preceding block.
    // Prefer the last heading starting at or before the line; else the first
    // heading after it.
    let headingBefore = -1;
    let headingAfter = -1;
    for (let i = 0; i < blockLines.length; i += 1) {
      if (!blockLines[i]!.isHeading) continue;
      if (blockLines[i]!.start <= line) headingBefore = i;
      else if (headingAfter < 0) headingAfter = i;
    }
    if (headingBefore >= 0) return headingBefore;
    return headingAfter;
  }

  useImperativeHandle(ref, () => {
    const jump = (block: number, offset: number, id: string | null, settled?: ScrollSettlement) => {
      navigation.start({
        virtualized: virtualize, offset, settled,
        mount: () => { if (virtualize) virtualizer.scrollToIndex(block, { align: 'start' }); },
        // Replaces the virtualizer's outstanding index reconciliation with the
        // real container offset, so it cannot later undo padding correction.
        place: virtualize ? offset => virtualizer.scrollToOffset(offset, { align: 'start', behavior: 'auto' }) : undefined,
        stop: virtualize ? () => virtualizer.scrollToOffset(scrollRef.current?.scrollTop ?? 0, { behavior: 'auto' }) : undefined,
        element: () => {
          const element = scrollRef.current?.querySelector<HTMLElement>(`[data-block-index="${block}"]`);
          return id ? Array.from(element?.querySelectorAll<HTMLElement>('[id]') ?? []).find(node => node.id === id) ?? null
            : element ?? null;
        },
      });
    };
    return {
      scrollToFragment(fragment: string, settled?: ScrollSettlement) {
        const target = fragmentTarget(blocks, fragment);
        if (!target) { navigation.cancel(); settled?.(false); return false; }
        jump(target.block, 0, target.id, settled);
        return true;
      },
      scrollToLine(line: number, settled?: ScrollSettlement) {
        const index = findBlockIndex(line);
        if (index < 0) { navigation.cancel(); settled?.(false); return; }
        jump(index, 0, null, settled);
      },
      getScrollAnchor() { return readAnchor(scrollRef.current); },
      scrollToBlock(block: number, offset: number, settled?: ScrollSettlement) {
        const clamped = Math.min(Math.max(block, 0), Math.max(blocks.length - 1, 0));
        jump(clamped, Math.max(0, Number.isFinite(offset) ? offset : 0), null, settled);
      },
      cancelScrollNavigation: navigation.cancel,
    };
  }, [virtualize, blockLines, virtualizer, blocks, navigation]);

  // Report the scroll anchor to the host as the reader scrolls (rAF-throttled).
  const handleScroll = useCallback(() => {
    if (!onScrollAnchorChange || navigation.active) return;
    if (scrollRaf.current !== null) return;
    scrollRaf.current = requestAnimationFrame(() => {
      scrollRaf.current = null;
      if (navigation.active) return;
      const anchor = readAnchor(scrollRef.current);
      if (anchor) onScrollAnchorChange(anchor);
    });
  }, [onScrollAnchorChange, navigation]);

  useEffect(
    () => () => {
      if (scrollRaf.current !== null) cancelAnimationFrame(scrollRaf.current);
    },
    [],
  );

  function isHighlighted(index: number): boolean {
    if (!highlightRange) return false;
    const { start, end } = blockLines[index]!;
    return start <= highlightRange.end && end >= highlightRange.start;
  }

  /**
   * A block's class list. The highlight itself is an inline `background`, which
   * no class-based oracle — the macOS AX harness reads `AXDOMClassList` — can
   * see, so the *located* blocks additionally carry a stable class. That is
   * what lets an end-to-end test assert which lines a click-to-source jump
   * actually landed on, rather than stopping at the span handed to `onJump`:
   * a jump whose end line was dropped along the way marks one block where it
   * should mark the whole range, and only a read of the FINAL state catches it.
   */
  function blockClass(index: number): string {
    return isHighlighted(index) ? 'mdreader-block mdreader-block--located' : 'mdreader-block';
  }

  /**
   * `aria-current` for the blocks a click-to-source jump landed on.
   *
   * Without it the jump is announced in colour and nothing else: the located
   * range is an inline `background`, so a reader using assistive technology is
   * moved to a passage with no indication that this is where they were taken —
   * information carried by colour alone. `location` is what the value means
   * ("the current location within a set"), and it is set only while the mark is
   * up, so nothing is announced as current once it fades.
   *
   * It is also what makes the located class above READABLE. A block wrapper is
   * a generic `<div>` whose children are all block-level, and WebKit leaves
   * those out of the accessibility tree entirely — class list and all — so the
   * macOS harness could see neither the mark nor its extent until the blocks
   * carrying it became elements at all.
   */
  function blockCurrent(index: number): 'location' | undefined {
    return isHighlighted(index) ? 'location' : undefined;
  }

  const contextValue = useMemo(
    () => ({
      onNavigate,
      platform,
      activeSlug: effectiveActiveSlug,
      resolveWikiLink,
      registerHeading,
    }),
    [onNavigate, platform, effectiveActiveSlug, resolveWikiLink, registerHeading],
  );

  return (
    <ReaderProvider value={contextValue}>
      <div
        ref={scrollRef}
        className="mdreader"
        onScroll={handleScroll}
        onPointerDownCapture={navigation.cancel}
        onTouchStartCapture={navigation.cancel}
        onWheelCapture={navigation.cancel}
        onKeyDownCapture={(event) => {
          if (['ArrowUp', 'ArrowDown', 'PageUp', 'PageDown', 'Home', 'End', ' '].includes(event.key)) navigation.cancel();
        }}
        style={{
          height: '100%',
          overflow: 'auto',
          fontFamily: 'var(--font-reading-serif)',
          fontSize: 'var(--text-0)',
          lineHeight: 'var(--line-height-body)',
          color: 'var(--color-fg)',
          background: 'var(--color-bg)',
        }}
      >
        {/*
         * The reading column. `margin: 0 auto` is the ONLY thing centring the
         * prose, and it only bites while `max-width` is smaller than the space
         * available — so the cap matters as much as the measure.
         *
         * `--measure` is `70ch`, and `ch` resolves against this element's own
         * font-size, which the host re-declares for the text-size setting: at the
         * larger steps the column grew until it filled the window, the clamp went
         * inert, and the prose sat against its left padding. `min()` with the
         * root-relative `--measure-cap` keeps a scale-independent ceiling under
         * it. The fallback keeps the component honest standing alone, without the
         * @mdreader/config tokens loaded.
         *
         * Padding goes through `--reader-pad` so a host can retune the column's
         * air (focus mode does) — an author stylesheet cannot, because this
         * inline declaration outranks it.
         */}
        <div
          className="mdreader__measure"
          style={{
            maxWidth: 'min(var(--measure), var(--measure-cap, 100%))',
            margin: '0 auto',
            padding: 'var(--reader-pad, var(--space-6))',
          }}
        >
          {doc.frontmatter ? <FrontmatterCard frontmatter={doc.frontmatter} /> : null}

          {virtualize ? (
            <div style={{ height: virtualizer.getTotalSize(), position: 'relative' }}>
              {virtualizer.getVirtualItems().map((item) => (
                <div
                  key={item.key}
                  data-block-index={item.index}
                  data-index={item.index}
                  className={blockClass(item.index)}
                  aria-current={blockCurrent(item.index)}
                  ref={virtualizer.measureElement}
                  style={{
                    position: 'absolute',
                    top: 0,
                    left: 0,
                    width: '100%',
                    transform: `translateY(${item.start}px)`,
                    background: isHighlighted(item.index)
                      ? 'var(--color-accent-soft)'
                      : undefined,
                  }}
                >
                  {renderedBlocks[item.index]}
                </div>
              ))}
            </div>
          ) : (
            renderedBlocks.map((block, index) => (
              <div
                key={index}
                data-block-index={index}
                className={blockClass(index)}
                aria-current={blockCurrent(index)}
                style={{
                  background: isHighlighted(index) ? 'var(--color-accent-soft)' : undefined,
                }}
              >
                {block}
              </div>
            ))
          )}
        </div>
      </div>
    </ReaderProvider>
  );
}
