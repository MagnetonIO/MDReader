import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, test, vi } from 'vitest';

import { SearchPalette, type SearchPaletteProps } from './search-palette.js';
import type { RelatedHit, RelatedOutcome, SearchAdapter, SearchHit, SearchOutcome } from './model.js';

beforeEach(() => {
  // cmdk scrolls the active item into view and observes its size; jsdom has
  // neither, so stub both.
  Element.prototype.scrollIntoView = vi.fn();
  globalThis.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;
});
afterEach(cleanup);

function hit(partial: Partial<SearchHit> & { chunkId: string }): SearchHit {
  return {
    documentId: 'doc',
    path: `/ws/${partial.chunkId}.md`,
    filePath: `${partial.chunkId}.md`,
    name: `${partial.chunkId}.md`,
    headingPath: [],
    startLine: 1,
    endLine: 2,
    score: 0.5,
    rrfScore: 0.4,
    boosts: { title: 0, path: 0, recency: 0, graph: 0 },
    lexRank: null,
    semRank: null,
    snippet: 'plain snippet',
    mtime: 0,
    explain: null,
    ...partial,
  };
}

/** A search adapter that resolves both phases from a fixed list. */
function staticAdapter(hits: SearchHit[]): SearchAdapter {
  return {
    search: vi.fn((_q, _o, onPhase) => {
      onPhase('lexical', hits);
      onPhase('final', hits);
      return Promise.resolve({ hits, semanticUnavailable: null });
    }),
    related: vi.fn(async () => []),
  };
}

function renderPalette(overrides: Partial<SearchPaletteProps> = {}) {
  const props: SearchPaletteProps = {
    mode: 'search',
    files: [],
    activeDoc: { path: '/ws/index.md', name: 'index.md' },
    sourceName: 'my-vault',
    adapter: staticAdapter([]),
    isMac: true,
    onOpenHit: vi.fn(),
    onOpenFile: vi.fn(),
    onClose: vi.fn(),
    reducedMotion: true,
    debounceMs: 0,
    ...overrides,
  };
  return { props, ...render(<SearchPalette {...props} />) };
}

describe('intents', () => {
  test('renders the five intents and clicking one re-searches in its mode', async () => {
    const adapter = staticAdapter([]);
    renderPalette({ adapter });

    for (const label of ['Best', 'Exact', 'Semantic', 'Recent', 'Related']) {
      expect(screen.getByRole('tab', { name: label })).toBeTruthy();
    }

    // Type a non-empty query first: an empty query is short-circuited to the
    // "Type to search" empty state and never reaches the adapter, so switching
    // intent only re-searches when there is something to search for.
    fireEvent.change(screen.getByRole('combobox'), { target: { value: 'pluggable' } });

    // Exact → lexical_only search.
    fireEvent.click(screen.getByRole('tab', { name: 'Exact' }));
    await waitFor(() =>
      expect(
        (adapter.search as ReturnType<typeof vi.fn>).mock.calls.some(
          (call) => call[1].lexicalOnly === true,
        ),
      ).toBe(true),
    );
    expect(screen.getByRole('tab', { name: 'Exact' }).getAttribute('aria-selected')).toBe('true');
  });

  test('Semantic sends a distinct intent and never presents lexical fallback as semantic results', async () => {
    const lexical = hit({ chunkId: 'literal', name: 'literal.md', lexRank: 1 });
    let finish: ((outcome: SearchOutcome) => void) | undefined;
    const adapter: SearchAdapter = {
      search: vi.fn((_query, _opts, onPhase) => {
        onPhase('lexical', [lexical]);
        onPhase('final', [lexical]);
        return new Promise<SearchOutcome>((resolve) => { finish = resolve; });
      }),
      related: vi.fn(async () => []),
    };
    renderPalette({ adapter });
    fireEvent.click(screen.getByRole('tab', { name: 'Semantic' }));
    fireEvent.change(screen.getByRole('combobox'), { target: { value: 'meaning' } });
    await waitFor(() => expect(adapter.search).toHaveBeenCalledWith(
      'meaning',
      { lexicalOnly: false, explain: true, intent: 'semantic' },
      expect.any(Function),
    ));
    expect(screen.queryByRole('option')).toBeNull();
    finish!({ hits: [lexical], semanticUnavailable: 'model unavailable' });
    const notice = await screen.findByRole('status');
    expect(notice.textContent).toContain('Semantic search is unavailable');
    expect(notice.textContent).not.toContain('Text matches only');
    expect(screen.queryByRole('option')).toBeNull();
  });

  test('Semantic shows actual vector-ranked evidence and excludes lexical-only hits', async () => {
    const vector = hit({ chunkId: 'meaning', name: 'meaning.md', semRank: 1 });
    const lexical = hit({ chunkId: 'literal', name: 'literal.md', lexRank: 1 });
    renderPalette({ adapter: staticAdapter([vector, lexical]) });
    fireEvent.click(screen.getByRole('tab', { name: 'Semantic' }));
    fireEvent.change(screen.getByRole('combobox'), { target: { value: 'paraphrase' } });
    await waitFor(() => expect(screen.getAllByRole('option')).toHaveLength(1));
    expect(screen.getByRole('option').getAttribute('data-value')).toBe('meaning');
    expect(screen.queryByRole('status')).toBeNull();
  });

  test('Best, Exact, and Recent preserve distinct intent in the adapter request', async () => {
    const adapter = staticAdapter([]);
    renderPalette({ adapter });
    fireEvent.change(screen.getByRole('combobox'), { target: { value: 'storage' } });
    for (const [label, intent, lexicalOnly] of [
      ['Best', 'best', false],
      ['Exact', 'exact', true],
      ['Recent', 'recent', false],
    ] as const) {
      fireEvent.click(screen.getByRole('tab', { name: label }));
      await waitFor(() => expect(adapter.search).toHaveBeenLastCalledWith(
        'storage', { lexicalOnly, explain: true, intent }, expect.any(Function),
      ));
    }
  });

  test('⌘1..5 selects intents and arrow keys rove the tablist', async () => {
    renderPalette();
    const input = screen.getByRole('combobox');
    expect(screen.getByRole('tab', { name: 'Best' }).getAttribute('aria-selected')).toBe('true');

    // ⌘2 → Exact (isMac is true in these tests).
    fireEvent.keyDown(input, { key: '2', metaKey: true });
    await waitFor(() =>
      expect(screen.getByRole('tab', { name: 'Exact' }).getAttribute('aria-selected')).toBe('true'),
    );

    // ArrowRight within the tablist → Semantic.
    fireEvent.keyDown(screen.getByRole('tab', { name: 'Exact' }), { key: 'ArrowRight' });
    await waitFor(() =>
      expect(screen.getByRole('tab', { name: 'Semantic' }).getAttribute('aria-selected')).toBe(
        'true',
      ),
    );

    // Bare Tab does NOT change the intent (it moves focus instead).
    fireEvent.keyDown(input, { key: 'Tab' });
    expect(screen.getByRole('tab', { name: 'Semantic' }).getAttribute('aria-selected')).toBe('true');
  });

  test('Related intent lists related() of the active document, decomposed', async () => {
    const related: RelatedHit[] = [
      {
        documentId: 'r1',
        path: '/ws/storage.md',
        filePath: 'storage.md',
        name: 'storage.md',
        score: 1.4,
        components: { graph: 0.9, concept: 0.3, vector: 0.2 },
      },
    ];
    const adapter: SearchAdapter = {
      search: vi.fn(() => Promise.resolve({ hits: [], semanticUnavailable: null })),
      related: vi.fn(async () => related),
    };
    renderPalette({ adapter });

    fireEvent.click(screen.getByRole('tab', { name: 'Related' }));
    await waitFor(() => expect(screen.getByRole('option')).toBeTruthy());
    expect(adapter.related).toHaveBeenCalledWith('/ws/index.md');
    // The decomposition chips reflect the non-zero components.
    expect(screen.getByText('graph')).toBeTruthy();
    expect(screen.getByText('concept')).toBeTruthy();
    expect(screen.getByText('vector')).toBeTruthy();
    // The score badge carries the component breakdown in its tooltip.
    expect(screen.getByText('1.40').getAttribute('title')).toContain('graph 0.90');
  });
  test('Related keeps coverage visible when no matches exist', async () => {
    renderPalette({ adapter: {
      search: vi.fn(async () => ({ hits: [], semanticUnavailable: null })),
      related: vi.fn(async (): Promise<RelatedOutcome> => ({ hits: [], availability: {
        vector: 'no_anchor_vectors', anchorChunks: 2, anchorVectors: 0,
        legacyConcepts: 'not_migrated', legacyGraphEntities: 'not_migrated',
      } })),
    } });
    fireEvent.click(screen.getByRole('tab', { name: 'Related' }));
    await waitFor(() => expect(screen.getByRole('status').textContent).toContain('Vector matches are unavailable'));
  });

});

describe('filter errors', () => {
  test('a schema-derived filter error renders inline under the input', async () => {
    // The real message Tauri delivers: EngineError::Query's Display is `{0}`, so
    // catdb-query's UnknownField format reaches the palette verbatim (no prefix).
    // Each help row is `FieldSpec::help_line`'s `{name}{alias}: {backing}`
    // (catdb-query vocab.rs), indented two spaces under the header.
    const adapter: SearchAdapter = {
      search: vi.fn(() =>
        Promise.reject(
          new Error(
            'unknown filter field `bogus`; valid fields (derived from the registered schemas):\n  path: markdown.Document.path\n  tag (alias tags): note.Doc → note.Concept.name',
          ),
        ),
      ),
      related: vi.fn(async () => []),
    };
    renderPalette({ adapter });
    fireEvent.change(screen.getByRole('combobox'), { target: { value: 'bogus:x' } });
    await waitFor(() => {
      const alert = screen.getByRole('alert');
      expect(alert.textContent).toContain('unknown filter field `bogus`');
    });
  });

  test('a punctuation-only query the engine rejects is softened (no error banner)', async () => {
    // The REAL string Tauri delivers for a no-searchable-terms rejection:
    // EngineError::Config's Display prepends "configuration error: " to the
    // inner `query "…" contains no searchable terms` (verified against
    // catdb-engine error.rs + search.rs and src-tauri/search.rs → fs.ts). That
    // is not a filter mistake to spell out — soften it to the empty state.
    const adapter: SearchAdapter = {
      search: vi.fn(() =>
        Promise.reject(
          new Error('configuration error: query "!!!" contains no searchable terms'),
        ),
      ),
      related: vi.fn(async () => []),
    };
    renderPalette({ adapter });
    fireEvent.change(screen.getByRole('combobox'), { target: { value: '!!!' } });
    await waitFor(() => expect(adapter.search).toHaveBeenCalled());
    // Softened to the "No results" empty state — no raw configuration-error banner.
    await waitFor(() => expect(screen.getByText('No results')).toBeTruthy());
    expect(screen.queryByRole('alert')).toBeNull();
  });

  test('a parse error that merely echoes "no searchable terms" is NOT softened away', async () => {
    // An UnterminatedQuote parse error echoes the raw input, so its message can
    // CONTAIN "contains no searchable terms" without BEING the engine's no-terms
    // Config error. It is delivered WITHOUT the "configuration error: " prefix
    // (EngineError::Query Display is `{0}`), so it does not match the anchored
    // softener and must still surface inline.
    const adapter: SearchAdapter = {
      search: vi.fn(() =>
        Promise.reject(
          new Error('unterminated quote in query: "\\"contains no searchable terms"'),
        ),
      ),
      related: vi.fn(async () => []),
    };
    renderPalette({ adapter });
    fireEvent.change(screen.getByRole('combobox'), {
      target: { value: '"contains no searchable terms' },
    });
    await waitFor(() => {
      const alert = screen.getByRole('alert');
      expect(alert.textContent).toContain('unterminated quote');
    });
  });
});

describe('empty query', () => {
  // Real bug: opening the palette empty (or clearing it) fired `search("")`,
  // which the engine rejects as `query "" contains no searchable terms`,
  // painting a red error banner on top of the "Type to search" empty state.
  test('an empty query never searches and shows no error banner', async () => {
    const adapter: SearchAdapter = {
      // Mimic what Tauri would actually deliver for an empty text query — the
      // prefixed EngineError::Config Display — so this sentinel stays faithful
      // even though the guard means it is never reached.
      search: vi.fn(() =>
        Promise.reject(new Error('configuration error: query "" contains no searchable terms')),
      ),
      related: vi.fn(async () => []),
    };
    renderPalette({ adapter });

    // The palette opens with an empty query. Give the debounce window time to
    // (not) fire and any rejection time to settle, then confirm we never sent
    // an empty query to the engine and never surfaced its error.
    await new Promise((resolve) => setTimeout(resolve, 40));
    expect(adapter.search).not.toHaveBeenCalled();
    expect(screen.queryByRole('alert')).toBeNull();
    expect(screen.getByText('Type to search')).toBeTruthy();
  });

  test('typing then clearing the input stops searching and clears the error (the user path)', async () => {
    const adapter: SearchAdapter = {
      search: vi.fn((query: string, _opts, onPhase) => {
        // Non-empty searches; empty is rejected as Tauri would deliver it (the
        // prefixed EngineError::Config Display).
        if (query.trim().length === 0) {
          return Promise.reject(
            new Error('configuration error: query "" contains no searchable terms'),
          );
        }
        onPhase('lexical', []);
        onPhase('final', []);
        return Promise.resolve({ hits: [], semanticUnavailable: null });
      }),
      related: vi.fn(async () => []),
    };
    renderPalette({ adapter });

    const input = screen.getByRole('combobox');
    fireEvent.change(input, { target: { value: 'pluggable' } });
    await waitFor(() =>
      expect(adapter.search).toHaveBeenCalledWith(
        'pluggable',
        expect.anything(),
        expect.anything(),
      ),
    );

    // Now clear the input (the reported path): this must not fire `search("")`
    // nor surface the engine's "no searchable terms" banner.
    fireEvent.change(input, { target: { value: '' } });
    await new Promise((resolve) => setTimeout(resolve, 40));

    const emptyCalls = (adapter.search as ReturnType<typeof vi.fn>).mock.calls.filter(
      (call) => (call[0] as string).trim().length === 0,
    );
    expect(emptyCalls).toHaveLength(0);
    expect(screen.queryByRole('alert')).toBeNull();
  });
});

describe('streamed merge ordering', () => {
  test('renders the lexical batch first, then reorders as the final pass lands', async () => {
    const a = hit({ chunkId: 'aaaa', name: 'alpha.md', score: 0.9 });
    const b = hit({ chunkId: 'bbbb', name: 'beta.md', score: 0.4 });
    const adapter: SearchAdapter = {
      search: vi.fn((_q, _o, onPhase) => {
        onPhase('lexical', [a, b]); // lexical order: alpha, beta
        return new Promise<SearchOutcome>((resolve) => {
          setTimeout(() => {
            onPhase('final', [b, a]); // hybrid reorders: beta, alpha
            resolve({ hits: [b, a], semanticUnavailable: null });
          }, 10);
        });
      }),
      related: vi.fn(async () => []),
    };
    renderPalette({ adapter });
    fireEvent.change(screen.getByRole('combobox'), { target: { value: 'x' } });

    await waitFor(() =>
      expect(screen.getAllByRole('option').map((el) => el.getAttribute('data-value'))).toEqual([
        'aaaa',
        'bbbb',
      ]),
    );
    await waitFor(() =>
      expect(screen.getAllByRole('option').map((el) => el.getAttribute('data-value'))).toEqual([
        'bbbb',
        'aaaa',
      ]),
    );
  });
});

describe('a ranking that is not the one asked for', () => {
  /**
   * The failure this guards: a hybrid search whose vector arm could not run
   * returns a shorter, differently-ordered list that is INDISTINGUISHABLE from a
   * full ranking on screen. The adapter reports the reason; the palette must say
   * it, next to the very results it qualifies.
   *
   * Revert-RED: stop rendering the `mdsearch__degraded` band (or drop
   * `setDegraded(outcome.semanticUnavailable)`) and this fails — the lexical-only
   * list renders with nothing marking it as such.
   */
  test('a lexical-only result set says so, and still shows its hits', async () => {
    const only = hit({ chunkId: 'lex1', name: 'storage.md' });
    const adapter: SearchAdapter = {
      search: vi.fn((_q, _o, onPhase) => {
        onPhase('lexical', [only]);
        onPhase('final', [only]);
        return Promise.resolve({
          hits: [only],
          semanticUnavailable: 'the embedding model could not be loaded — timed out after 120s',
        });
      }),
      related: vi.fn(async () => []),
    };
    renderPalette({ adapter });
    fireEvent.change(screen.getByRole('combobox'), { target: { value: 'storage' } });

    // Non-emptiness first: the hits ARE shown — degrading is not hiding.
    await waitFor(() => expect(screen.getAllByRole('option').length).toBe(1));

    const notice = await waitFor(() => screen.getByRole('status'));
    expect(notice.textContent).toContain('semantic ranking is unavailable');
    expect(notice.textContent).toContain('the embedding model could not be loaded');
    // It is a qualification, not an error: the error banner stays absent.
    expect(screen.queryByRole('alert')).toBeNull();
  });

  test('a full hybrid ranking says nothing at all', async () => {
    renderPalette({ adapter: staticAdapter([hit({ chunkId: 'lex1' })]) });
    fireEvent.change(screen.getByRole('combobox'), { target: { value: 'storage' } });
    await waitFor(() => expect(screen.getAllByRole('option').length).toBe(1));
    expect(screen.queryByRole('status')).toBeNull();
  });
});

describe('ResultRow', () => {
  test('renders title, highlighted snippet, score badge, and provenance breadcrumb', async () => {
    const h = hit({
      chunkId: 'c1',
      name: 'deep.md',
      filePath: 'notes/deep.md',
      headingPath: ['Top', 'Storage'],
      snippet: 'the [storage] engine persists chunks',
      score: 1.23,
      rrfScore: 0.8,
      boosts: { title: 0.1, path: 0, recency: 0, graph: 0 },
    });
    renderPalette({ adapter: staticAdapter([h]) });
    fireEvent.change(screen.getByRole('combobox'), { target: { value: 'storage' } });

    await waitFor(() => expect(screen.getByText('deep.md')).toBeTruthy());
    // Highlighted snippet term rendered as <mark>, not raw markup.
    const mark = screen.getByText('storage', { selector: 'mark' });
    expect(mark).toBeTruthy();
    // Score badge + explain tooltip.
    const badge = screen.getByText('1.23');
    expect(badge.getAttribute('title')).toContain('rrf 0.80');
    // Provenance breadcrumb: source › file › heading path.
    expect(screen.getByText('my-vault')).toBeTruthy();
    expect(screen.getByRole('button', { name: 'notes/deep.md' })).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Storage' })).toBeTruthy();
  });
});

describe('opening hits', () => {
  test('Enter opens the hit at its provenance line; ⌘Enter opens a background tab', async () => {
    const onOpenHit = vi.fn();
    const onClose = vi.fn();
    const h = hit({ chunkId: 'c1', name: 'deep.md', startLine: 7, sourceId: 'provider:' + 'a'.repeat(64), revision: 'b'.repeat(64) });
    renderPalette({ adapter: staticAdapter([h]), onOpenHit, onClose });
    fireEvent.change(screen.getByRole('combobox'), { target: { value: 'x' } });

    const option = await screen.findByRole('option');
    // ⌘Enter → background tab, palette stays open.
    fireEvent.keyDown(screen.getByRole('combobox'), { key: 'Enter', metaKey: true });
    await waitFor(() =>
      expect(onOpenHit).toHaveBeenCalledWith(
        expect.objectContaining({ path: '/ws/c1.md', line: 7, sourceId: h.sourceId, revision: h.revision }),
        { background: true },
      ),
    );
    expect(onClose).not.toHaveBeenCalled();

    // Plain Enter (click) → foreground open at the provenance line, palette closes.
    fireEvent.click(option);
    await waitFor(() =>
      expect(onOpenHit).toHaveBeenCalledWith(
        expect.objectContaining({ path: '/ws/c1.md', line: 7, sourceId: h.sourceId, revision: h.revision }),
        { background: false },
      ),
    );
    expect(onClose).toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: 'c1.md' }));
    expect(onOpenHit).toHaveBeenLastCalledWith(expect.objectContaining({path:h.path,sourceId:h.sourceId,revision:h.revision,line:undefined}),{background:false});
  });
});

describe('modal a11y', () => {
  test('is a labelled aria-modal dialog with a keyboard-reachable breadcrumb', async () => {
    const h = hit({
      chunkId: 'c1',
      name: 'deep.md',
      filePath: 'notes/deep.md',
      headingPath: ['Storage'],
      snippet: 'x',
    });
    renderPalette({ adapter: staticAdapter([h]) });
    fireEvent.change(screen.getByRole('combobox'), { target: { value: 'x' } });
    await screen.findByRole('option');

    const dialog = screen.getByRole('dialog', { name: 'Search' });
    expect(dialog.getAttribute('aria-modal')).toBe('true');

    // The provenance breadcrumb file/heading segments are real, focusable buttons.
    const crumb = screen.getByRole('button', { name: 'notes/deep.md' });
    crumb.focus();
    expect(document.activeElement).toBe(crumb);
  });

  test('traps Tab within the overlay (wraps at the ends)', async () => {
    const h = hit({ chunkId: 'c1', name: 'deep.md', filePath: 'notes/deep.md', snippet: 'x' });
    renderPalette({ adapter: staticAdapter([h]) });
    fireEvent.change(screen.getByRole('combobox'), { target: { value: 'x' } });
    await screen.findByRole('option');

    const dialog = screen.getByRole('dialog', { name: 'Search' });
    const focusable = dialog.querySelectorAll<HTMLElement>(
      'button:not([tabindex="-1"]), input:not([disabled]), [tabindex]:not([tabindex="-1"])',
    );
    const first = focusable[0]!;
    const last = focusable[focusable.length - 1]!;
    expect(first).not.toBe(last);

    // Shift+Tab from the first focusable wraps to the last.
    first.focus();
    fireEvent.keyDown(first, { key: 'Tab', shiftKey: true });
    expect(document.activeElement).toBe(last);

    // Tab from the last focusable wraps back to the first.
    last.focus();
    fireEvent.keyDown(last, { key: 'Tab' });
    expect(document.activeElement).toBe(first);
  });
});

describe('workspace switch (stale-result belt)', () => {
  test('clears results the instant the workspaceKey changes', async () => {
    const { props, rerender } = renderPalette({
      workspaceKey: '/wsA',
      adapter: staticAdapter([hit({ chunkId: 'aaaa', name: 'alpha.md' })]),
    });
    fireEvent.change(screen.getByRole('combobox'), { target: { value: 'x' } });
    await waitFor(() => expect(screen.getAllByRole('option')).toHaveLength(1));

    // The SAME palette instance re-keyed to a NEW workspace whose search returns
    // nothing: the previous workspace's hit must not linger.
    rerender(<SearchPalette {...props} workspaceKey="/wsB" adapter={staticAdapter([])} />);
    await waitFor(() => expect(screen.queryAllByRole('option')).toHaveLength(0));
  });
});

describe('file mode', () => {
  test('ranks and opens files (the unified Cmd+P jump)', async () => {
    const onOpenFile = vi.fn();
    renderPalette({
      mode: 'file',
      files: [
        { path: '/ws/guide.md', name: 'guide.md' },
        { path: '/ws/index.md', name: 'index.md' },
      ],
      onOpenFile,
    });
    // No intents in file mode.
    expect(screen.queryByRole('tab', { name: 'Best' })).toBeNull();
    fireEvent.change(screen.getByRole('combobox'), { target: { value: 'guide' } });
    const option = await screen.findByText('guide.md');
    fireEvent.click(option.closest('[role="option"]')!);
    expect(onOpenFile).toHaveBeenCalledWith(
      expect.objectContaining({ name: 'guide.md' }),
      { background: false },
    );
  });
});
