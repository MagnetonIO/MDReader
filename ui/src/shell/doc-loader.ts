/**
 * The ReaderPane's document load + parse lifecycle, extracted so its
 * cancellation is workspace-aware AND testable. The effect keys on `path`,
 * `reloadNonce`, AND `root`: a workspace switch (root change) with the same tab
 * still mounted reruns cleanup, so an in-flight parse for the OLD workspace is
 * cancelled rather than landing under the new one. (Root-in-deps is chosen over
 * keying the whole ReaderPane by root — it discards only the stale parse without
 * remounting the reader and losing scroll/outline state.)
 */
import { useEffect, useRef, useState } from 'react';
import type { ParsedDoc } from '@mdreader/markdown';

export interface LoadState {
  status: 'idle' | 'loading' | 'ready' | 'error';
  doc?: ParsedDoc;
  dir?: string;
  error?: string;
}

export interface DocLoaderDeps {
  /** Active document path (null → idle). */
  path: string | null;
  /** Granted workspace root — a change reruns cleanup (workspace switch). */
  root: string | null;
  /** Live-reload nonce — a change re-reads the same document. */
  reloadNonce: number;
  /** Read the document's text content. */
  read: (path: string) => Promise<string>;
  /** Parse content into a rendered doc. */
  parse: (content: string) => Promise<ParsedDoc>;
  /** Directory of a path (for relative-link resolution). */
  dirOf: (path: string) => string;
  /** Word count of the content (for the status bar). */
  wordsOf: (content: string) => number;
  /** Publish doc metadata (or null on idle/error). */
  onMeta: (meta: { words: number } | null) => void;
}

export function useDocLoader(deps: DocLoaderDeps): LoadState {
  const { path, root, reloadNonce, read, parse, dirOf, wordsOf, onMeta } = deps;
  const [state, setState] = useState<LoadState>({ status: 'idle' });
  // The root the last render loaded under, so a workspace switch can be told
  // apart from a same-workspace reload / doc change.
  const prevRootRef = useRef(root);

  useEffect(() => {
    if (!path) {
      setState({ status: 'idle' });
      onMeta(null);
      prevRootRef.current = root;
      return;
    }
    let cancelled = false;
    // A ROOT change (workspace switch/reload) must not keep the previous
    // workspace's document on screen while the new one loads — drop it to a
    // clean loading state (#d). A same-workspace reload / doc switch keeps the
    // prior doc mounted (spread) so live-reload doesn't flash empty.
    const rootChanged = prevRootRef.current !== root;
    prevRootRef.current = root;
    if (rootChanged) setState({ status: 'loading' });
    else setState((prev) => ({ ...prev, status: 'loading' }));
    void (async () => {
      try {
        const content = await read(path);
        const doc = await parse(content);
        if (cancelled) return;
        setState({ status: 'ready', doc, dir: dirOf(path) });
        onMeta({ words: wordsOf(content) });
      } catch (cause) {
        if (cancelled) return;
        setState({ status: 'error', error: cause instanceof Error ? cause.message : String(cause) });
        onMeta(null);
      }
    })();
    return () => {
      cancelled = true;
    };
    // read/parse/dirOf/wordsOf/onMeta are stable (memoized by the caller);
    // `path` + `reloadNonce` + `root` drive reloads / workspace-switch cleanup.
  }, [path, reloadNonce, root]);

  return state;
}
