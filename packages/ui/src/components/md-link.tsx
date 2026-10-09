'use client';

import type { MouseEvent, ReactNode } from 'react';

import { useReaderContext } from '../reader-context.js';

export interface MdLinkProps {
  href?: string;
  children?: ReactNode;
  id?: string;
  title?: string;
  'aria-label'?: string;
  'aria-describedby'?: string;
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
export function MdLink({ href = '', children, id, title, 'aria-label': ariaLabel, 'aria-describedby': ariaDescribedBy }: MdLinkProps) {
  const { onNavigate, platform } = useReaderContext();
  const labels = { title, 'aria-label': ariaLabel, 'aria-describedby': ariaDescribedBy };

  if (href.startsWith('#')) {
    let slug = href.slice(1);
    try { slug = decodeURIComponent(slug); } catch { /* An unmatched malformed fragment remains visibly unavailable. */ }
    const activate = (event: MouseEvent<HTMLAnchorElement>) => {
      if (event.type === 'auxclick' && event.button !== 1) return;
      if (onNavigate) {
        event.preventDefault();
        onNavigate({ kind: 'anchor', slug });
      }
    };
    return (
      <a
        href={href}
        id={id}
        {...labels}
        draggable={false}
        style={{ color: 'var(--color-accent)' }}
        onClick={activate}
        onAuxClick={activate}
      >
        {children}
      </a>
    );
  }

  if (isExternal(href)) {
    const external = href.startsWith('//') ? `https:${href}` : href;
    if (!/^(?:https?:|mailto:|tel:)/i.test(external)) return <a id={id} {...labels} aria-disabled="true">{children}</a>;
    function handleExternal(event: MouseEvent<HTMLAnchorElement>) {
      if (event.type === 'auxclick' && event.button !== 1) return;
      if (platform) {
        event.preventDefault();
        platform.openExternal(external);
      }
    }
    return (
      <a
        href={external}
        id={id}
        {...labels}
        target="_blank"
        rel="noreferrer noopener"
        draggable={false}
        style={{ color: 'var(--color-accent)' }}
        onClick={handleExternal}
        onAuxClick={handleExternal}
      >
        {children}
      </a>
    );
  }

  const resolved = platform ? platform.resolveAssetUrl(href) : href;
  function handleRelative(event: MouseEvent<HTMLAnchorElement>) {
    if (event.type === 'auxclick' && event.button !== 1) return;
    if (onNavigate) {
      event.preventDefault();
      onNavigate({ kind: 'relative', href });
    }
  }
  return (
    <a
      href={resolved}
      id={id}
      {...labels}
      draggable={false}
      style={{ color: 'var(--color-accent)' }}
      onClick={handleRelative}
      onAuxClick={handleRelative}
    >
      {children}
    </a>
  );
}
