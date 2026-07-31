'use client';

import { useEffect, useRef, useState } from 'react';
import type { ReactNode } from 'react';

import { useReaderContext } from '../reader-context.js';

type HeadingLevel = 1 | 2 | 3 | 4 | 5 | 6;
type HeadingTag = 'h1' | 'h2' | 'h3' | 'h4' | 'h5' | 'h6';

export interface HeadingProps {
  id?: string;
  children?: ReactNode;
  'data-line-start'?: string;
  'data-line-end'?: string;
}

/**
 * A heading (h1–h6) with an anchor link revealed on hover and its `id` kept from
 * rehype-slug so in-document anchors and the outline line up. The active outline
 * slug is emphasized.
 *
 * The anchor sits in the left gutter (`left: -1.1em`) — exactly where a reader
 * starts a left-to-right selection drag. It is faded out rather than removed, so
 * it must be hidden from the POINTER as well as from the eye: no engine begins a
 * text selection from inside an `<a>`, so an invisible-but-hit-testable anchor
 * silently ate every drag that started in a heading's gutter. `pointerEvents`
 * therefore tracks `revealed` in lockstep with `opacity` — an invisible control
 * is never a target — and `draggable={false}` keeps the revealed anchor from
 * offering a drag payload nothing can use.
 *
 * It stays in the DOM and in the accessibility tree throughout: `pointer-events`
 * does not affect keyboard focus or screen readers, and keyboard focus reveals it
 * (the previous hover-only reveal left a focused anchor invisible).
 */
export function makeHeading(level: HeadingLevel) {
  const Tag = `h${level}` as HeadingTag;

  function Heading({ id, children, ...rest }: HeadingProps) {
    const [hovered, setHovered] = useState(false);
    const [focused, setFocused] = useState(false);
    const revealed = hovered || focused;
    const { activeSlug, registerHeading } = useReaderContext();
    const isActive = Boolean(id) && id === activeSlug;
    const ref = useRef<HTMLHeadingElement | null>(null);

    // Register this heading with the reader on mount so scroll-spy tracks the
    // real rendered DOM (and unregister on unmount / id change).
    useEffect(() => {
      if (!id || !registerHeading || !ref.current) return;
      return registerHeading(id, ref.current);
    }, [id, registerHeading]);

    return (
      <Tag
        ref={ref}
        id={id}
        data-line-start={rest['data-line-start']}
        data-line-end={rest['data-line-end']}
        onMouseEnter={() => setHovered(true)}
        onMouseLeave={() => setHovered(false)}
        style={{
          position: 'relative',
          fontFamily: 'var(--font-reading-sans)',
          lineHeight: 'var(--line-height-heading)',
          scrollMarginTop: 'var(--space-6)',
          color: isActive ? 'var(--color-accent)' : 'inherit',
        }}
      >
        {id ? (
          <a
            href={`#${id}`}
            aria-label="Link to this section"
            className="mdreader-heading__anchor"
            draggable={false}
            onFocus={() => setFocused(true)}
            onBlur={() => setFocused(false)}
            style={{
              position: 'absolute',
              left: '-1.1em',
              opacity: revealed ? 1 : 0,
              pointerEvents: revealed ? 'auto' : 'none',
              textDecoration: 'none',
              color: 'var(--color-faint)',
            }}
          >
            #
          </a>
        ) : null}
        {children}
      </Tag>
    );
  }

  Heading.displayName = `Heading${level}`;
  return Heading;
}
