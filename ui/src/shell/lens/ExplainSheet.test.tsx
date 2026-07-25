/**
 * Query Explanation.
 *
 * The whole derivation has to be present — both arms' ranks, the fused RRF
 * score, every boost's contribution, the final ordering key, the index and
 * model the semantic arm searched, and the full provenance chain. And the
 * absences have to stay absences: a boost that never fired is not a
 * contribution of zero, and an arm that never ran is not a rank of zero.
 */

import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, test, vi } from 'vitest';

import type { ExplainedHit, SemanticsAdapter } from '../../platform/semantics.js';
import { ExplainSheet } from './ExplainSheet.js';

afterEach(cleanup);

function hit(overrides: Partial<ExplainedHit> = {}): ExplainedHit {
  return {
    chunkId: 'chunk-aaaa1111-2222',
    documentId: 'doc-bbbb1111-2222',
    filePath: 'notes/adr-3.md',
    absPath: '/ws/notes/adr-3.md',
    name: 'adr-3.md',
    headingPath: ['Storage', 'Why SQLite'],
    startLine: 12,
    endLine: 30,
    snippet: 'we adopt SQLite',
    score: 0.0421,
    explain: {
      lexRank: 2,
      semRank: 1,
      rrfScore: 0.0325,
      boosts: { title: 0.006, path: null, recency: 0.0036, graph: null },
      finalScore: 0.0421,
      indexVersion: 'idx-7',
      embeddingModel: 'bge-small-en',
      provenance: {
        chunkId: 'chunk-aaaa1111-2222',
        sectionId: 'sect-cccc1111-2222',
        documentId: 'doc-bbbb1111-2222',
        filePath: 'notes/adr-3.md',
        sourceId: 'src-dddd1111-2222',
        sourceRoot: '/ws',
      },
    },
    ...overrides,
  };
}

function adapterWith(hits: ExplainedHit[]): SemanticsAdapter {
  return {
    listSchemas: vi.fn(),
    projectCorpus: vi.fn(),
    describeSchema: vi.fn(),
    inspectSpan: vi.fn(),
    approveFact: vi.fn(),
    rejectFact: vi.fn(),
    checkConstraints: vi.fn(),
    explainQuery: vi.fn(async () => hits),
  } as SemanticsAdapter;
}

async function runQuery(adapter: SemanticsAdapter, onOpen = vi.fn()) {
  render(<ExplainSheet adapter={adapter} initialQuery="sqlite" onOpen={onOpen} onClose={vi.fn()} />);
  fireEvent.click(screen.getByRole('button', { name: 'Explain' }));
  await waitFor(() => expect(adapter.explainQuery).toHaveBeenCalledWith('sqlite'));
  return onOpen;
}

describe('the ranking, shown in full', () => {
  test('names both arms with their ranks, the RRF, and the final score', async () => {
    await runQuery(adapterWith([hit()]));

    expect(await screen.findByText('Both arms matched, and their ranks were fused.')).toBeTruthy();
    expect(screen.getByText('lexical #2 · semantic #1')).toBeTruthy();
    expect(screen.getByText('0.0325')).toBeTruthy(); // the fused RRF, before boosts
    expect(screen.getAllByText('0.0421').length).toBeGreaterThan(0); // the ordering key
  });

  test('a boost that never fired says so — it is NOT a contribution of zero', async () => {
    await runQuery(adapterWith([hit()]));

    // Path and graph never fired; title and recency did.
    expect(await screen.findAllByText('did not fire')).toHaveLength(2);
    expect(screen.getByText('0.0060')).toBeTruthy();
    expect(screen.getByText('0.0036')).toBeTruthy();
    // The absent ones are not rendered as 0.0000.
    expect(screen.queryByText('0.0000')).toBeNull();
  });

  test('a hit found by only one arm says which arm missed', async () => {
    await runQuery(
      adapterWith([hit({ explain: { ...hit().explain!, semRank: null } })]),
    );

    expect(await screen.findByText('Only the lexical arm matched this chunk.')).toBeTruthy();
    expect(screen.getByText('lexical #2 · semantic did not match')).toBeTruthy();
  });

  test('names the index version and embedding model the semantic arm searched', async () => {
    await runQuery(adapterWith([hit()]));
    expect(await screen.findByText('idx-7 · bge-small-en')).toBeTruthy();
  });

  test('a semantic arm that never ran is not given a phantom index', async () => {
    await runQuery(
      adapterWith([
        hit({ explain: { ...hit().explain!, indexVersion: null, semRank: null } }),
      ]),
    );
    expect(await screen.findByText('the semantic arm did not run')).toBeTruthy();
  });
});

describe('the semantic path and the provenance chain', () => {
  test('shows where in the document the matched chunk sits', async () => {
    await runQuery(adapterWith([hit()]));
    expect(await screen.findByText(/Storage › Why SQLite/)).toBeTruthy();
    expect(screen.getByText(/lines 12–30/)).toBeTruthy();
  });

  test('shows chunk → section → document → file → source, end to end', async () => {
    await runQuery(adapterWith([hit()]));

    const chain = await screen.findByLabelText('Provenance chain');
    expect(chain.textContent).toContain('chunk chunk-aa…');
    expect(chain.textContent).toContain('section sect-ccc…');
    expect(chain.textContent).toContain('document doc-bbbb…');
    expect(chain.textContent).toContain('notes/adr-3.md');
    expect(chain.textContent).toContain('source src-dddd…');
  });

  test('an unattributed hit says there is no source, rather than showing a blank link', async () => {
    await runQuery(
      adapterWith([
        hit({ explain: { ...hit().explain!, provenance: { ...hit().explain!.provenance, sourceId: null } } }),
      ]),
    );
    expect(await screen.findByText('no attributing source')).toBeTruthy();
  });

  test('opening a hit lands on its section’s first line', async () => {
    const onOpen = await runQuery(adapterWith([hit()]));
    fireEvent.click(await screen.findByRole('button', { name: 'adr-3.md' }));
    expect(onOpen).toHaveBeenCalledWith(expect.objectContaining({ absPath: '/ws/notes/adr-3.md', startLine: 12 }));
  });
});

describe('nothing to explain', () => {
  test('an empty result set says there is no ranking, not that something failed', async () => {
    await runQuery(adapterWith([]));
    expect(await screen.findByText('Nothing matched. There is no ranking to explain.')).toBeTruthy();
  });

  test('a blank query cannot be run', () => {
    const adapter = adapterWith([]);
    render(<ExplainSheet adapter={adapter} onOpen={vi.fn()} onClose={vi.fn()} />);
    const explain = screen.getByRole<HTMLButtonElement>('button', { name: 'Explain' });
    expect(explain.disabled).toBe(true);
    fireEvent.click(explain);
    expect(adapter.explainQuery).not.toHaveBeenCalled();
  });
});
