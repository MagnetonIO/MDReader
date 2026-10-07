import { act, cleanup, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, test, vi } from 'vitest';
import type { RelatedHit, SearchAdapter } from '@mdreader/ui';

import { RelatedRail } from './RelatedRail.js';

afterEach(cleanup);

function relatedHit(name: string, comps: { graph: number; concept: number; vector: number }): RelatedHit {
  return {
    documentId: `doc:${name}`,
    path: `/ws/${name}`,
    filePath: name,
    name,
    score: comps.graph + comps.concept + comps.vector,
    components: comps,
  };
}

function adapterWith(related: SearchAdapter['related']): SearchAdapter {
  return { search: vi.fn(), related };
}

describe('RelatedRail', () => {
  test('debounces rapid active-doc changes to a SINGLE fetch for the final doc', () => {
    vi.useFakeTimers();
    try {
      const related = vi.fn(async () => []);
      const base = {
        adapter: adapterWith(related),
        open: true,
        onToggle: vi.fn(),
        onOpen: vi.fn(),
        workspaceKey: '/ws',
      };
      const { rerender } = render(<RelatedRail {...base} activeDoc={{ path: '/ws/a.md', name: 'a.md' }} />);
      rerender(<RelatedRail {...base} activeDoc={{ path: '/ws/b.md', name: 'b.md' }} />);
      rerender(<RelatedRail {...base} activeDoc={{ path: '/ws/c.md', name: 'c.md' }} />);

      // Still inside the 250ms debounce window: NO fetch yet (would be 3 already
      // if the debounce were removed — this line fails on a reverted debounce).
      act(() => vi.advanceTimersByTime(200));
      expect(related).not.toHaveBeenCalled();

      // Past the window: exactly ONE fetch, for the FINAL doc only.
      act(() => vi.advanceTimersByTime(100));
      expect(related).toHaveBeenCalledTimes(1);
      expect(related).toHaveBeenCalledWith('/ws/c.md');
    } finally {
      vi.useRealTimers();
    }
  });

  test('renders the score decomposition (tooltip + chips)', async () => {
    const related = vi.fn(async () => [relatedHit('storage.md', { graph: 0.5, concept: 0.3, vector: 0.2 })]);
    render(
      <RelatedRail
        activeDoc={{ path: '/ws/a.md', name: 'a.md' }}
        adapter={adapterWith(related)}
        open
        onToggle={vi.fn()}
        onOpen={vi.fn()}
        workspaceKey="/ws"
      />,
    );

    await waitFor(() => expect(screen.getByRole('button', { name: /storage\.md/ })).toBeTruthy());
    expect(related).toHaveBeenCalledWith('/ws/a.md');

    // The score badge tooltip carries the graph/concept/vector decomposition.
    const link = screen.getByRole('button', { name: /storage\.md/ });
    expect(link.getAttribute('title')).toContain('graph 0.50');
    expect(link.getAttribute('title')).toContain('concept 0.30');
    expect(link.getAttribute('title')).toContain('vector 0.20');
    // All three components are non-zero, so all three chips render.
    expect(link.querySelectorAll('.related-rail__chip')).toHaveLength(3);
  });

  test('shows real shared tags and unavailable vectors without calling tags concepts', async () => {
    const hit = relatedHit('tagged.md', { graph: 0, concept: 0, vector: 0 });
    hit.components = { graph: 0, concept: 0, sharedTags: 0.5, vector: null };
    hit.score = 0.5;
    render(<RelatedRail activeDoc={{ path: '/ws/a.md', name: 'a.md' }}
      adapter={adapterWith(async () => ({ hits: [hit], availability: {
        vector: 'no_anchor_vectors', anchorChunks: 1, anchorVectors: 0,
        legacyConcepts: 'not_migrated', legacyGraphEntities: 'not_migrated',
      } }))} open onToggle={vi.fn()} onOpen={vi.fn()} workspaceKey="/ws" />);
    const row = await screen.findByRole('button', { name: /tagged.md/ });
    expect(row.title).toContain('shared tags 0.50');
    expect(row.title).toContain('vector unavailable');
    expect(row.title).not.toContain('concept');
    expect(screen.getByRole('status').textContent).toContain('Vector matches are unavailable');
    expect(screen.getByRole('status').textContent).toContain('not migrated');
  });

  test('empty results still disclose incomplete coverage', async () => {
    render(<RelatedRail activeDoc={{ path: '/ws/a.md', name: 'a.md' }}
      adapter={adapterWith(async () => ({ hits: [], availability: {
        vector: 'available', anchorChunks: 5, anchorVectors: 2,
        legacyConcepts: 'not_migrated', legacyGraphEntities: 'not_migrated',
      } }))} open onToggle={vi.fn()} onOpen={vi.fn()} workspaceKey="/ws" />);
    await screen.findByText('No related notes yet.');
    expect(screen.getByRole('status').textContent).toContain('2 of 5');
  });

  test('opening a related row navigates to it', async () => {
    const onOpen = vi.fn();
    render(
      <RelatedRail
        activeDoc={{ path: '/ws/a.md', name: 'a.md' }}
        adapter={adapterWith(async () => [relatedHit('graph.md', { graph: 0.4, concept: 0, vector: 0 })])}
        open
        onToggle={vi.fn()}
        onOpen={onOpen}
        workspaceKey="/ws"
      />,
    );
    const link = await screen.findByRole('button', { name: /graph\.md/ });
    // Only the graph component is non-zero → one chip.
    expect(link.querySelectorAll('.related-rail__chip')).toHaveLength(1);
    link.click();
    expect(onOpen).toHaveBeenCalledWith({ path: '/ws/graph.md', name: 'graph.md' });
  });

  test('opening a ranked related match preserves its source line', async () => {
    const onOpen = vi.fn();
    const hit = { ...relatedHit('evidence.md', { graph: 0, concept: 0, vector: 0.4 }), line: 37 };
    render(<RelatedRail activeDoc={{ path: '/ws/a.md', name: 'a.md' }}
      adapter={adapterWith(async () => [hit])} open onToggle={vi.fn()} onOpen={onOpen} workspaceKey="/ws" />);
    (await screen.findByRole('button', { name: /evidence.md/ })).click();
    expect(onOpen).toHaveBeenCalledWith({ path: '/ws/evidence.md', name: 'evidence.md', line: 37 });
  });

  test('collapsed: shows a toggle and does not fetch', () => {
    const related = vi.fn(async () => []);
    render(
      <RelatedRail
        activeDoc={{ path: '/ws/a.md', name: 'a.md' }}
        adapter={adapterWith(related)}
        open={false}
        onToggle={vi.fn()}
        onOpen={vi.fn()}
        workspaceKey="/ws"
      />,
    );
    expect(screen.getByTitle('Show related notes')).toBeTruthy();
    expect(related).not.toHaveBeenCalled();
  });

  test('a workspace change re-fetches for the new workspace (the guard)', async () => {
    const related = vi.fn(async () => []);
    const props = {
      activeDoc: { path: '/ws/a.md', name: 'a.md' },
      adapter: adapterWith(related),
      open: true,
      onToggle: vi.fn(),
      onOpen: vi.fn(),
    };
    const { rerender } = render(<RelatedRail {...props} workspaceKey="/wsA" />);
    await waitFor(() => expect(related).toHaveBeenCalledTimes(1));

    // A workspaceKey change re-runs the fetch effect (dropping any in-flight
    // result for the previous workspace).
    rerender(<RelatedRail {...props} workspaceKey="/wsB" />);
    await waitFor(() => expect(related).toHaveBeenCalledTimes(2));
  });
});
