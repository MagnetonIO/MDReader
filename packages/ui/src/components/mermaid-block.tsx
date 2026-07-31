'use client';

import { useEffect, useRef, useState } from 'react';

import { ErrorBoundary } from './error-boundary.js';

// Pinned mermaid theme names keyed by reader theme.
const MERMAID_THEME = { light: 'default', dark: 'dark' } as const;

export interface MermaidBlockProps {
  /** The raw mermaid diagram source. */
  source: string;
  dataLineStart?: string;
  dataLineEnd?: string;
}

let diagramCounter = 0;

function currentTheme(): 'light' | 'dark' {
  if (typeof document !== 'undefined') {
    const attr = document.documentElement.getAttribute('data-theme');
    if (attr === 'dark' || attr === 'light') return attr;
  }
  if (typeof matchMedia !== 'undefined' && matchMedia('(prefers-color-scheme: dark)').matches) {
    return 'dark';
  }
  return 'light';
}

/** The plain source fallback, shown before render and on failure. */
function SourceFallback({
  source,
  dataLineStart,
  dataLineEnd,
}: MermaidBlockProps) {
  return (
    <pre
      className="mdreader-mermaid__source"
      data-line-start={dataLineStart}
      data-line-end={dataLineEnd}
      style={{
        margin: 'var(--space-5) 0',
        padding: 'var(--space-3)',
        border: '1px solid var(--color-border)',
        borderRadius: 'var(--radius-md)',
        background: 'var(--color-code-bg)',
        fontFamily: 'var(--font-mono)',
        fontSize: 'var(--text--1)',
        overflowX: 'auto',
      }}
    >
      <code>{source}</code>
    </pre>
  );
}

/**
 * Renders a Mermaid diagram. Mermaid is imported lazily the first time the block
 * scrolls into view (IntersectionObserver); the pinned mermaid version renders
 * to SVG. Failures fall back to the diagram source (both via async catch and an
 * {@link ErrorBoundary}). Re-renders when the reader theme changes.
 */
export function MermaidBlock(props: MermaidBlockProps) {
  const { source, dataLineStart, dataLineEnd } = props;
  const containerRef = useRef<HTMLDivElement | null>(null);
  const [visible, setVisible] = useState(typeof IntersectionObserver === 'undefined');
  const [theme, setTheme] = useState<'light' | 'dark'>(currentTheme);
  const [svg, setSvg] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);

  // Reveal on first intersection.
  useEffect(() => {
    if (visible || typeof IntersectionObserver === 'undefined') return;
    const element = containerRef.current;
    if (!element) return;
    const observer = new IntersectionObserver((entries) => {
      if (entries.some((entry) => entry.isIntersecting)) {
        setVisible(true);
        observer.disconnect();
      }
    });
    observer.observe(element);
    return () => observer.disconnect();
  }, [visible]);

  // Track theme changes so the diagram can re-render. Mermaid renders to static
  // SVG with a theme baked in at render time, so both the system preference
  // (`prefers-color-scheme`) and a manual toggle (which stamps `[data-theme]`
  // onto <html>) must trigger a re-render.
  useEffect(() => {
    const update = () => setTheme(currentTheme());

    let media: MediaQueryList | undefined;
    if (typeof matchMedia !== 'undefined') {
      media = matchMedia('(prefers-color-scheme: dark)');
      media.addEventListener('change', update);
    }

    let observer: MutationObserver | undefined;
    if (typeof MutationObserver !== 'undefined' && typeof document !== 'undefined') {
      observer = new MutationObserver(update);
      observer.observe(document.documentElement, {
        attributes: true,
        attributeFilter: ['data-theme'],
      });
    }

    return () => {
      media?.removeEventListener('change', update);
      observer?.disconnect();
    };
  }, []);

  // Render (or re-render) the diagram once visible / when theme changes.
  useEffect(() => {
    if (!visible) return;
    let active = true;
    setFailed(false);
    void (async () => {
      try {
        const mermaid = (await import('mermaid')).default;
        mermaid.initialize({ startOnLoad: false, theme: MERMAID_THEME[theme] });
        const id = `mdreader-mermaid-${(diagramCounter += 1)}`;
        const { svg: rendered } = await mermaid.render(id, source);
        if (active) setSvg(rendered);
      } catch {
        if (active) {
          setSvg(null);
          setFailed(true);
        }
      }
    })();
    return () => {
      active = false;
    };
  }, [visible, theme, source]);

  if (failed) {
    return <SourceFallback {...props} />;
  }

  const sharedProps = {
    ref: containerRef,
    className: 'mdreader-mermaid',
    'data-line-start': dataLineStart,
    'data-line-end': dataLineEnd,
    style: { margin: 'var(--space-5) 0', textAlign: 'center' as const },
  };

  return (
    <ErrorBoundary fallback={<SourceFallback {...props} />}>
      {svg ? (
        <div {...sharedProps} dangerouslySetInnerHTML={{ __html: svg }} />
      ) : (
        <div {...sharedProps}>
          <SourceFallback {...props} />
        </div>
      )}
    </ErrorBoundary>
  );
}
