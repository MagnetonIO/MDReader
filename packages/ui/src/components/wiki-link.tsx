'use client';

import type { MouseEvent, ReactNode } from 'react';

import { useReaderContext } from '../reader-context.js';

export interface WikiLinkProps {
  children?: ReactNode;
  'data-target'?: string;
  'data-heading'?: string;
  'data-label'?: string;
}

/**
 * Renders a `[[wikilink]]`. Clicking emits an `onNavigate` event with the
 * target and heading; the host owns actual resolution/navigation. When the
 * reader context provides a resolver, the link is marked `data-resolved` so
 * unresolved links can be styled distinctly.
 *
 * `draggable={false}` suppresses the native link drag-and-drop, which for an
 * `href="#"` wikilink carries a payload that means nothing outside this app: a
 * drag off a wikilink would otherwise raise a drag ghost and offer the OS a `#`
 * URL to drop somewhere. Measured in both engines: without it `dragstart` fires
 * on the anchor; with it, nothing does.
 *
 * What it does NOT do — measured, in a bare page, in Chromium and WebKit alike —
 * is make a drag that STARTS inside a link select text. No engine selects there,
 * with or without `draggable`/`-webkit-user-drag`; that is platform behaviour
 * every web page shares, not something this reader should fight. A drag that
 * merely CROSSES a link or an image has always selected correctly.
 */
export function WikiLink(props: WikiLinkProps) {
  const target = props['data-target'] ?? '';
  const heading = props['data-heading'] ?? null;
  const { onNavigate, resolveWikiLink } = useReaderContext();
  const resolved = resolveWikiLink ? resolveWikiLink(target) : undefined;

  function handleClick(event: MouseEvent<HTMLAnchorElement>) {
    event.preventDefault();
    onNavigate?.({ kind: 'wikilink', target, heading });
  }

  return (
    <a
      href="#"
      className="mdreader-wikilink"
      data-target={target}
      data-heading={heading ?? undefined}
      data-resolved={resolved === undefined ? undefined : String(resolved)}
      draggable={false}
      onClick={handleClick}
      style={{
        color: resolved === false ? 'var(--color-danger)' : 'var(--color-accent)',
        textDecoration: resolved === false ? 'underline dotted' : 'underline',
        textUnderlineOffset: '2px',
        cursor: 'pointer',
      }}
    >
      {props.children}
    </a>
  );
}
