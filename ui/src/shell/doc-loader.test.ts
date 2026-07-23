import { act, renderHook, waitFor } from '@testing-library/react';
import { describe, expect, test, vi } from 'vitest';
import type { ParsedDoc } from '@mdreader/markdown';

import { useDocLoader } from './doc-loader.js';

const fakeDoc = (marker: string): ParsedDoc =>
  ({ hast: { type: 'root', children: [] }, frontmatter: null, outline: [{ depth: 1, text: marker, slug: marker, lineStart: 1 }], stats: { blockCount: 0, byteLength: 0 } }) as unknown as ParsedDoc;

describe('useDocLoader — workspace awareness', () => {
  test('a root switch mid-parse discards the OLD workspace parse', async () => {
    // A parser we can resolve on demand, per invocation.
    const pending: Array<(doc: ParsedDoc) => void> = [];
    const parse = vi.fn(
      () => new Promise<ParsedDoc>((resolve) => pending.push(resolve)),
    );
    const read = vi.fn(async () => 'content');
    const onMeta = vi.fn();

    const base = {
      path: '/note.md', // SAME tab stays mounted across the switch
      reloadNonce: 0,
      read,
      parse,
      dirOf: (p: string) => p,
      wordsOf: () => 1,
      onMeta,
    };

    const { result, rerender } = renderHook((props) => useDocLoader(props), {
      initialProps: { ...base, root: '/wsA' },
    });

    await waitFor(() => expect(parse).toHaveBeenCalledTimes(1)); // parse under /wsA started
    expect(result.current.status).toBe('loading');

    // Switch workspace (root change) with the same tab mounted → cleanup reruns,
    // a fresh parse starts under /wsB.
    rerender({ ...base, root: '/wsB' });
    await waitFor(() => expect(parse).toHaveBeenCalledTimes(2));

    // The OLD (/wsA) parse resolves LATE — it must be discarded (cancelled), so
    // its doc never becomes the ready state.
    await act(async () => {
      pending[0]!(fakeDoc('wsA-doc'));
      await Promise.resolve();
    });
    expect(result.current.status).toBe('loading'); // NOT ready with the stale doc

    // The NEW (/wsB) parse resolving DOES land.
    await act(async () => {
      pending[1]!(fakeDoc('wsB-doc'));
      await Promise.resolve();
    });
    await waitFor(() => expect(result.current.status).toBe('ready'));
    expect(result.current.doc?.outline[0]?.text).toBe('wsB-doc');
  });
});
