'use client';

import type { OutlineItem } from '@mdreader/markdown';

export interface OutlineProps {
  /** Headings to list (typically `doc.outline`). */
  items: OutlineItem[];
  /** Slug of the currently active heading, emphasized in the rail. */
  activeSlug?: string;
  /** Invoked when a heading is selected. */
  onSelect?: (item: OutlineItem) => void;
}

/**
 * The right-rail document outline. Purely presentational: it lists headings,
 * indents by depth, highlights the active slug, and reports selections.
 */
export function Outline({ items, activeSlug, onSelect }: OutlineProps) {
  if (items.length === 0) return null;

  const minDepth = Math.min(...items.map((item) => item.depth));

  return (
    <nav
      className="mdreader-outline"
      aria-label="Document outline"
      style={{ fontFamily: 'var(--font-ui)', fontSize: 'var(--text--1)', lineHeight: 1.4 }}
    >
      <ul style={{ listStyle: 'none', margin: 0, padding: 0 }}>
        {items.map((item) => {
          const isActive = item.slug === activeSlug;
          return (
            <li key={`${item.slug}-${item.lineStart}`}>
              <a
                href={`#${item.slug}`}
                aria-current={isActive ? 'location' : undefined}
                onClick={() => onSelect?.(item)}
                style={{
                  display: 'block',
                  padding: 'var(--space-1) var(--space-2)',
                  paddingLeft: `calc(var(--space-2) + ${(item.depth - minDepth) * 0.85}rem)`,
                  color: isActive ? 'var(--color-accent)' : 'var(--color-muted)',
                  fontWeight: isActive ? 600 : 400,
                  textDecoration: 'none',
                  borderLeft: isActive
                    ? '2px solid var(--color-accent)'
                    : '2px solid transparent',
                }}
              >
                {item.text}
              </a>
            </li>
          );
        })}
      </ul>
    </nav>
  );
}
