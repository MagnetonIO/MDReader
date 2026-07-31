'use client';

import { useState } from 'react';
import { isFrontmatterError } from '@mdreader/markdown';
import type { FrontmatterData } from '@mdreader/markdown';

export interface FrontmatterCardProps {
  frontmatter: FrontmatterData;
  /** Whether the card starts collapsed (default true). */
  defaultCollapsed?: boolean;
}

function formatValue(value: unknown): string {
  if (Array.isArray(value)) return value.map(formatValue).join(', ');
  if (value === null) return 'null';
  if (typeof value === 'object') return JSON.stringify(value);
  return String(value);
}

/**
 * A collapsed key/value card for a document's frontmatter. Renders the malformed
 * `{ raw, error }` case with a subtle parse warning while still showing the raw
 * block, so nothing is silently dropped.
 */
export function FrontmatterCard({ frontmatter, defaultCollapsed = true }: FrontmatterCardProps) {
  const [collapsed, setCollapsed] = useState(defaultCollapsed);
  const malformed = isFrontmatterError(frontmatter);
  const entries = malformed ? [] : Object.entries(frontmatter);

  return (
    <aside
      className="mdreader-frontmatter"
      data-malformed={malformed ? 'true' : undefined}
      style={{
        margin: 'var(--space-5) 0',
        border: '1px solid var(--color-border)',
        borderRadius: 'var(--radius-md)',
        background: 'var(--color-surface)',
        fontFamily: 'var(--font-ui)',
        fontSize: 'var(--text--1)',
      }}
    >
      <button
        type="button"
        className="mdreader-frontmatter__toggle"
        aria-expanded={!collapsed}
        onClick={() => setCollapsed((value) => !value)}
        style={{
          display: 'flex',
          alignItems: 'center',
          gap: 'var(--space-2)',
          width: '100%',
          padding: 'var(--space-2) var(--space-3)',
          font: 'inherit',
          color: 'var(--color-muted)',
          background: 'transparent',
          border: 'none',
          cursor: 'pointer',
          textAlign: 'left',
        }}
      >
        <span aria-hidden>{collapsed ? '▸' : '▾'}</span>
        <span>Frontmatter</span>
        {malformed ? (
          <span
            className="mdreader-frontmatter__warning"
            style={{ color: 'var(--color-warning)' }}
          >
            · could not be parsed
          </span>
        ) : null}
      </button>

      {collapsed ? null : (
        <div className="mdreader-frontmatter__body" style={{ padding: '0 var(--space-3) var(--space-3)' }}>
          {malformed ? (
            <>
              <p style={{ margin: '0 0 var(--space-2)', color: 'var(--color-warning)' }}>
                {frontmatter.error}
              </p>
              <pre
                style={{
                  margin: 0,
                  padding: 'var(--space-2)',
                  background: 'var(--color-code-bg)',
                  borderRadius: 'var(--radius-sm)',
                  fontFamily: 'var(--font-mono)',
                  overflowX: 'auto',
                }}
              >
                <code>{frontmatter.raw}</code>
              </pre>
            </>
          ) : (
            <dl
              style={{
                display: 'grid',
                gridTemplateColumns: 'auto 1fr',
                gap: 'var(--space-1) var(--space-3)',
                margin: 0,
              }}
            >
              {entries.map(([key, value]) => (
                <div key={key} style={{ display: 'contents' }}>
                  <dt style={{ color: 'var(--color-muted)', fontWeight: 600 }}>{key}</dt>
                  <dd style={{ margin: 0 }}>{formatValue(value)}</dd>
                </div>
              ))}
            </dl>
          )}
        </div>
      )}
    </aside>
  );
}
