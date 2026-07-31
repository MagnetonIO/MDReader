'use client';

import type { MouseEvent, ReactNode } from 'react';

import { useReaderContext } from '../reader-context.js';

export interface MdLinkProps {
  href?: string;
  children?: ReactNode;
  id?: string;
}

/** Absolute (external) URL, incl. protocol-relative and mailto/tel. */
function isExternal(href: string): boolean {
  return /^(?:[a-z][a-z0-9+.-]*:|\/\/)/i.test(href);
}

/**
 * Renders a markdown link. In-document anchors (`#slug`) emit an `anchor`
 * navigation; external links open via the platform's `openExternal`; relative
 * links are resolved through `resolveAssetUrl` and emit a `relative`
 * navigation.
 *
 * Every variant is `draggable={false}`, to suppress the native link
 * drag-and-drop — see {@link WikiLink}, including what that does and does not
 * buy for text selection.
 */
export function MdLink({ href = '', children, id }: MdLinkProps) {
  const { onNavigate, platform } = useReaderContext();

  if (href.startsWith('#')) {
    const slug = href.slice(1);
    return (
      <a
        href={href}
        id={id}
        draggable={false}
        style={{ color: 'var(--color-accent)' }}
        onClick={() => onNavigate?.({ kind: 'anchor', slug })}
      >
        {children}
      </a>
    );
  }

  if (isExternal(href)) {
    function handleExternal(event: MouseEvent<HTMLAnchorElement>) {
      if (platform) {
        event.preventDefault();
        platform.openExternal(href);
      }
    }
    return (
      <a
        href={href}
        id={id}
        target="_blank"
        rel="noreferrer noopener"
        draggable={false}
        style={{ color: 'var(--color-accent)' }}
        onClick={handleExternal}
      >
        {children}
      </a>
    );
  }

  const resolved = platform ? platform.resolveAssetUrl(href) : href;
  function handleRelative(event: MouseEvent<HTMLAnchorElement>) {
    event.preventDefault();
    onNavigate?.({ kind: 'relative', href });
  }
  return (
    <a
      href={resolved}
      id={id}
      draggable={false}
      style={{ color: 'var(--color-accent)' }}
      onClick={handleRelative}
    >
      {children}
    </a>
  );
}
