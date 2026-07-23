import { cleanup, render, screen, waitFor } from '@testing-library/react';
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
  test('lazily fetches (debounced to one call) and renders the score decomposition', async () => {
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
    // One debounced call for the active doc.
    expect(related).toHaveBeenCalledTimes(1);
    expect(related).toHaveBeenCalledWith('/ws/a.md');

    // The score badge tooltip carries the graph/concept/vector decomposition.
    const link = screen.getByRole('button', { name: /storage\.md/ });
    expect(link.getAttribute('title')).toContain('graph 0.50');
    expect(link.getAttribute('title')).toContain('concept 0.30');
    expect(link.getAttribute('title')).toContain('vector 0.20');
    // All three components are non-zero, so all three chips render.
    expect(link.querySelectorAll('.related-rail__chip')).toHaveLength(3);
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
