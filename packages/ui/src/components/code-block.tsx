'use client';

import { Fragment, useEffect, useRef, useState } from 'react';
import { jsx, jsxs } from 'react/jsx-runtime';
import { toJsxRuntime } from 'hast-util-to-jsx-runtime';
import type { ReactNode } from 'react';

import { highlightToHast } from '../shiki-highlighter.js';

export interface CodeBlockProps {
  /** Raw source of the code fence. */
  code: string;
  /** Language id from the fence info string (may be empty). */
  lang: string;
  /** Provenance line stamps carried over from the source `<pre>`. */
  dataLineStart?: string;
  dataLineEnd?: string;
}

/**
 * A fenced code block. Renders a plain `<pre>` immediately (SSR-safe) and
 * upgrades to Shiki-highlighted output once the lazily-loaded highlighter
 * resolves. Includes a copy button and a language badge. Colors come from the
 * dual-theme Shiki output driven by the CSS theme tokens.
 */
export function CodeBlock({ code, lang, dataLineStart, dataLineEnd }: CodeBlockProps) {
  const [highlighted, setHighlighted] = useState<ReactNode | null>(null);
  const [copied, setCopied] = useState(false);
  const copyResetRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    let active = true;
    void highlightToHast(code, lang).then((tree) => {
      if (!active || !tree) return;
      setHighlighted(toJsxRuntime(tree, { Fragment, jsx, jsxs }));
    });
    return () => {
      active = false;
    };
  }, [code, lang]);

  useEffect(
    () => () => {
      if (copyResetRef.current) clearTimeout(copyResetRef.current);
    },
    [],
  );

  function copy() {
    void navigator.clipboard?.writeText(code).then(() => {
      setCopied(true);
      if (copyResetRef.current) clearTimeout(copyResetRef.current);
      copyResetRef.current = setTimeout(() => setCopied(false), 1500);
    });
  }

  return (
    <div
      className="mdreader-code"
      data-line-start={dataLineStart}
      data-line-end={dataLineEnd}
      data-lang={lang || undefined}
      style={{
        position: 'relative',
        margin: 'var(--space-5) 0',
        border: '1px solid var(--color-border)',
        borderRadius: 'var(--radius-md)',
        background: 'var(--color-code-bg)',
        overflow: 'hidden',
        fontFamily: 'var(--font-mono)',
        fontSize: 'var(--text--1)',
      }}
    >
      <div
        className="mdreader-code__bar"
        style={{
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'space-between',
          padding: 'var(--space-1) var(--space-3)',
          borderBottom: '1px solid var(--color-border)',
          color: 'var(--color-muted)',
          fontFamily: 'var(--font-ui)',
          fontSize: 'var(--text--1)',
        }}
      >
        <span className="mdreader-code__lang">{lang || 'text'}</span>
        <button
          type="button"
          className="mdreader-code__copy"
          onClick={copy}
          aria-label="Copy code"
          style={{
            font: 'inherit',
            color: 'inherit',
            background: 'transparent',
            border: '1px solid var(--color-border)',
            borderRadius: 'var(--radius-sm)',
            padding: '0 var(--space-2)',
            cursor: 'pointer',
          }}
        >
          {copied ? 'Copied' : 'Copy'}
        </button>
      </div>
      {highlighted ?? (
        <pre
          className="mdreader-code__pre"
          style={{ margin: 0, padding: 'var(--space-3)', overflowX: 'auto' }}
        >
          <code>{code}</code>
        </pre>
      )}
    </div>
  );
}
