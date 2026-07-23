/**
 * The reading surface for the active tab — and only the active tab is ever
 * mounted here, honouring the "one Reader" invariant. It reads the document
 * through the thin fs command layer, parses it with the real
 * `@mdreader/markdown` pipeline (off-thread for large docs via `parseMarkdownAuto`),
 * and renders the shared `MarkdownReader`. It also:
 *   - persists a scroll anchor (the active heading's source line) into the tab,
 *     restoring it on load / reopen / live-refresh;
 *   - drives the OutlineRail from scroll-spy and scrolls on outline selection;
 *   - resolves wikilinks/relative links against the workspace file list;
 *   - restyles per the reading-font / text-size settings without touching
 *     `@mdreader/ui` (it overrides the reader's CSS custom properties on a wrapper).
 */

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { CSSProperties } from 'react';
import { MarkdownReader, PlatformProvider } from '@mdreader/ui';
import type { MarkdownReaderHandle } from '@mdreader/ui';
import { parseMarkdownAuto } from '@mdreader/markdown/worker';
import type { OutlineItem, ParsedDoc } from '@mdreader/markdown';

import { readDoc } from '../platform/fs.js';
import type { WorkspaceFile } from '../platform/fs.js';
import { createReaderPlatform } from '../platform/reader-platform.js';
import { OutlineRail } from './OutlineRail.js';
import { ReaderToolbar } from './ReaderToolbar.js';
import type { ReadingFont } from '../stores/settings.js';
import type { DocRef, ScrollPos, TabState } from '../stores/types.js';

interface LoadState {
  status: 'idle' | 'loading' | 'ready' | 'error';
  doc?: ParsedDoc;
  dir?: string;
  error?: string;
}

export interface ReaderPaneProps {
  tab: TabState | null;
  root: string | null;
  files: WorkspaceFile[];
  outlineVisible: boolean;
  readingFont: ReadingFont;
  textScale: number;
  reloadNonce: number;
  onToggleOutline: () => void;
  onNavigate: (ref: DocRef) => void;
  onPersistScroll: (scroll: ScrollPos) => void;
  onDocMeta: (meta: { words: number } | null) => void;
  onPickFolder: () => void;
  onMissingWikiLink: (target: string) => void;
}

function dirname(path: string): string {
  const normalized = path.replace(/[\\/]+$/, '');
  const index = Math.max(normalized.lastIndexOf('/'), normalized.lastIndexOf('\\'));
  return index >= 0 ? normalized.slice(0, index) : normalized;
}

function countWords(content: string): number {
  const trimmed = content.trim();
  return trimmed.length === 0 ? 0 : trimmed.split(/\s+/).length;
}

function stripExtension(name: string): string {
  return name.replace(/\.(md|markdown)$/i, '');
}

/** Resolve a `[[wikilink]]` target to a workspace file (by basename). */
function findByWikiTarget(files: WorkspaceFile[], target: string): WorkspaceFile | null {
  const wanted = stripExtension(target.trim()).toLowerCase();
  return (
    files.find((file) => stripExtension(file.name).toLowerCase() === wanted) ??
    files.find((file) => stripExtension(file.name).toLowerCase().endsWith(wanted)) ??
    null
  );
}

function resolveRelative(files: WorkspaceFile[], docDir: string, href: string): WorkspaceFile | null {
  const clean = href.split('#')[0]!.split('?')[0]!;
  const base = docDir.replace(/[\\/]+$/, '');
  const stack = base.split(/[\\/]+/);
  for (const part of clean.split(/[\\/]+/)) {
    if (part === '' || part === '.') continue;
    if (part === '..') stack.pop();
    else stack.push(part);
  }
  const resolved = stack.join('/');
  return files.find((file) => file.path.replace(/\\/g, '/') === resolved) ?? null;
}

export function ReaderPane({
  tab,
  root,
  files,
  outlineVisible,
  readingFont,
  textScale,
  reloadNonce,
  onToggleOutline,
  onNavigate,
  onPersistScroll,
  onDocMeta,
  onPickFolder,
  onMissingWikiLink,
}: ReaderPaneProps) {
  const readerRef = useRef<MarkdownReaderHandle | null>(null);
  const [state, setState] = useState<LoadState>({ status: 'idle' });
  const [activeSlug, setActiveSlug] = useState<string | undefined>(undefined);

  const path = tab?.docRef.path ?? null;
  // The scroll position to restore for the freshly-loaded doc — captured in a
  // ref so scroll writes (which happen after restore) never retrigger a restore.
  const scrollToRestore = useRef<ScrollPos>({ block: 0, offset: 0 });
  scrollToRestore.current = tab?.scroll ?? { block: 0, offset: 0 };

  // Load + parse whenever the active document or a live-reload nonce changes.
  useEffect(() => {
    if (!path) {
      setState({ status: 'idle' });
      onDocMeta(null);
      return;
    }
    let cancelled = false;
    setState((prev) => ({ ...prev, status: 'loading' }));
    void (async () => {
      try {
        const { content } = await readDoc(path);
        const doc = await parseMarkdownAuto(content);
        if (cancelled) return;
        setState({ status: 'ready', doc, dir: dirname(path) });
        onDocMeta({ words: countWords(content) });
      } catch (cause) {
        if (cancelled) return;
        setState({ status: 'error', error: cause instanceof Error ? cause.message : String(cause) });
        onDocMeta(null);
      }
    })();
    return () => {
      cancelled = true;
    };
    // onDocMeta is stable (memoized by the caller); path + nonce drive reloads.
  }, [path, reloadNonce]);

  // Restore the block+offset scroll position once, after a fresh document
  // renders. Works for headingless docs and mid-block offsets alike (the
  // anchor is a block index + pixel offset, not a heading line).
  useEffect(() => {
    if (state.status !== 'ready') return;
    const { block, offset } = scrollToRestore.current;
    if (block <= 0 && offset <= 0) return;
    const frame = requestAnimationFrame(() => readerRef.current?.scrollToBlock(block, offset));
    return () => cancelAnimationFrame(frame);
    // Intentionally keyed on the loaded doc only, not the tab (scroll writes
    // must not retrigger a restore).
  }, [state.doc]);

  // Persist the scroll anchor, debounced, so store churn (and its re-renders)
  // stays off the scroll's hot path while still capturing the resting position.
  const persistTimer = useRef<number | undefined>(undefined);
  const persistScroll = useCallback(
    (anchor: ScrollPos) => {
      window.clearTimeout(persistTimer.current);
      persistTimer.current = window.setTimeout(() => onPersistScroll(anchor), 200);
    },
    [onPersistScroll],
  );
  useEffect(() => () => window.clearTimeout(persistTimer.current), []);

  const outline = state.doc?.outline ?? [];

  const handleActiveHeading = useCallback((slug: string | undefined) => {
    setActiveSlug(slug);
  }, []);

  const onOutlineSelect = useCallback((item: OutlineItem) => {
    readerRef.current?.scrollToLine(item.lineStart);
  }, []);

  const resolveWikiLink = useCallback(
    (target: string) => findByWikiTarget(files, target) !== null,
    [files],
  );

  const onReaderNavigate = useCallback(
    (event: { kind: string; target?: string; heading?: string | null; slug?: string; href?: string }) => {
      if (event.kind === 'wikilink' && event.target) {
        const match = findByWikiTarget(files, event.target);
        if (match) onNavigate({ path: match.path, name: match.name });
        else onMissingWikiLink(event.target);
      } else if (event.kind === 'anchor' && event.slug) {
        const item = outline.find((entry) => entry.slug === event.slug);
        if (item) readerRef.current?.scrollToLine(item.lineStart);
      } else if (event.kind === 'relative' && event.href && state.dir) {
        const match = resolveRelative(files, state.dir, event.href);
        if (match) onNavigate({ path: match.path, name: match.name });
      }
    },
    [files, outline, onNavigate, onMissingWikiLink, state.dir],
  );

  const platform = useMemo(() => createReaderPlatform(state.dir ?? null), [state.dir]);

  const hostStyle = useMemo(() => {
    const style: Record<string, string> = { ['--text-0']: `calc(1rem * ${textScale})` };
    if (readingFont === 'sans') style['--font-reading-serif'] = 'var(--font-reading-sans)';
    return style as CSSProperties;
  }, [textScale, readingFont]);

  if (!tab) {
    return (
      <div className="reader-region">
        <div className="reader-empty">
          <h2 className="reader-empty__title">A calm place to read your markdown</h2>
          <p className="reader-empty__hint">
            Open a workspace folder to browse its notes, then press a file — or use{' '}
            <span className="kbd">Cmd P</span> to jump straight to one.
          </p>
          {root ? null : (
            <button type="button" className="btn-primary" onClick={onPickFolder}>
              Open folder…
            </button>
          )}
        </div>
      </div>
    );
  }

  return (
    <div className="reader-region">
      <ReaderToolbar
        docRef={tab.docRef}
        root={root}
        outlineVisible={outlineVisible}
        onToggleOutline={onToggleOutline}
        hasOutline={outline.length > 0}
      />
      <div className="reader-body">
        <div className="reader-host" style={hostStyle}>
          {state.status === 'error' ? (
            <div className="reader-empty">
              <p className="reader-empty__hint">Could not open this document: {state.error}</p>
            </div>
          ) : state.doc ? (
            <PlatformProvider platform={platform}>
              <MarkdownReader
                ref={readerRef}
                doc={state.doc}
                activeSlug={activeSlug}
                onActiveHeadingChange={handleActiveHeading}
                onNavigate={onReaderNavigate}
                resolveWikiLink={resolveWikiLink}
                onScrollAnchorChange={persistScroll}
              />
            </PlatformProvider>
          ) : null}
        </div>
        {outlineVisible && state.doc ? (
          <OutlineRail items={outline} activeSlug={activeSlug} onSelect={onOutlineSelect} />
        ) : null}
      </div>
    </div>
  );
}
