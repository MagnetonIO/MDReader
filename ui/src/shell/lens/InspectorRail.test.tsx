/**
 * The Semantic Inspector, held to its promises.
 *
 * The two that matter most are refusals: a review may not be submitted without
 * a real reviewer, and a claim's recorded state may not be smoothed into
 * agreement with the graph. Both are asserted as behaviour — what the adapter
 * is and is not called with — rather than as text on screen.
 */

import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, describe, expect, test, vi } from 'vitest';

import type { InspectedFact, SemanticsAdapter } from '../../platform/semantics.js';
import { InspectorRail } from './InspectorRail.js';

afterEach(cleanup);

function fact(overrides: Partial<InspectedFact> = {}): InspectedFact {
  return {
    factId: 'fact-1',
    kind: 'object',
    schema: 'decision_overlay',
    name: 'decision_overlay.Decision',
    value: '{"title":"Adopt SQLite"}',
    sourceId: null,
    targetId: null,
    rule: 'D1',
    extractor: 'mdreader.decision@1',
    provenance: {
      file: 'notes/adr-3.md',
      startLine: 12,
      endLine: 18,
      contentHash: 'blake3:9f2c1a4e88b7d3c2ff01',
      schemaVersion: '4',
      extractor: 'mdreader.decision@1',
      confidence: 0.82,
      approvedBy: null,
    },
    state: 'proposed',
    reviewer: null,
    inGraph: false,
    reviewedContentHash: null,
    sourceChangedSinceReview: false,
    ...overrides,
  };
}

function adapterWith(facts: InspectedFact[], overrides: Partial<SemanticsAdapter> = {}): SemanticsAdapter {
  return {
    listSchemas: vi.fn(async () => []),
    projectCorpus: vi.fn(),
    describeSchema: vi.fn(),
    inspectSpan: vi.fn(async () => facts),
    approveFact: vi.fn(async (factId: string) => ({ factId, promoted: [factId] })),
    rejectFact: vi.fn(async (factId: string) => ({ factId, retracted: [], cascaded: [] })),
    checkConstraints: vi.fn(),
    explainQuery: vi.fn(),
    ...overrides,
  } as SemanticsAdapter;
}

const SPAN = { path: '/ws/notes/adr-3.md', startLine: 12, endLine: 18 };

function renderRail(
  adapter: SemanticsAdapter,
  props: Partial<React.ComponentProps<typeof InspectorRail>> = {},
) {
  return render(
    <InspectorRail
      adapter={adapter}
      open
      onToggle={vi.fn()}
      selection={SPAN}
      reviewer=""
      onSetReviewer={vi.fn()}
      onCite={vi.fn()}
      workspaceKey="/ws"
      {...props}
    />,
  );
}

describe('a review is signed', () => {
  test('a BLANK reviewer cannot submit — the verbs are inert and nothing is sent', async () => {
    const adapter = adapterWith([fact()]);
    renderRail(adapter, { reviewer: '' });

    const approve = await screen.findByRole<HTMLButtonElement>('button', { name: 'Approve' });
    const reject = screen.getByRole<HTMLButtonElement>('button', { name: 'Reject' });
    expect(approve.disabled).toBe(true);
    expect(reject.disabled).toBe(true);

    // Even forced, no placeholder reviewer reaches the backend.
    fireEvent.click(approve);
    fireEvent.click(reject);
    expect(adapter.approveFact).not.toHaveBeenCalled();
    expect(adapter.rejectFact).not.toHaveBeenCalled();
  });

  test('a WHITESPACE-ONLY name is not an identity: it neither saves nor unlocks', async () => {
    const onSetReviewer = vi.fn();
    const adapter = adapterWith([fact()]);
    renderRail(adapter, { reviewer: '   ', onSetReviewer });

    const input = await screen.findByLabelText(/who is ruling/i);
    fireEvent.change(input, { target: { value: '   ' } });

    expect(screen.getByRole<HTMLButtonElement>('button', { name: 'Save' }).disabled).toBe(true);
    // A whitespace `reviewer` prop is treated exactly as no reviewer at all.
    expect(screen.getByRole<HTMLButtonElement>('button', { name: 'Approve' }).disabled).toBe(true);

    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    expect(onSetReviewer).not.toHaveBeenCalled();
  });

  test('a real name is offered to the store and unlocks the verbs', async () => {
    const onSetReviewer = vi.fn();
    const adapter = adapterWith([fact()]);
    const { rerender } = renderRail(adapter, { reviewer: '', onSetReviewer });

    fireEvent.change(await screen.findByLabelText(/who is ruling/i), {
      target: { value: 'Ada Lovelace' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    expect(onSetReviewer).toHaveBeenCalledWith('Ada Lovelace');

    rerender(
      <InspectorRail
        adapter={adapter}
        open
        onToggle={vi.fn()}
        selection={SPAN}
        reviewer="Ada Lovelace"
        onSetReviewer={onSetReviewer}
        onCite={vi.fn()}
        workspaceKey="/ws"
      />,
    );
    await waitFor(() =>
      expect(screen.getByRole<HTMLButtonElement>('button', { name: 'Approve' }).disabled).toBe(false),
    );
    expect(screen.getByText('Ada Lovelace')).toBeTruthy();
  });
});

describe('ruling on a claim', () => {
  test('approving sends the fact id AND the reviewer, then re-reads the span', async () => {
    const adapter = adapterWith([fact()]);
    renderRail(adapter, { reviewer: 'Ada Lovelace' });

    const approve = await screen.findByRole('button', { name: 'Approve' });
    expect(adapter.inspectSpan).toHaveBeenCalledTimes(1);
    fireEvent.click(approve);

    await waitFor(() => expect(adapter.approveFact).toHaveBeenCalledWith('fact-1', 'Ada Lovelace'));
    // Whether the promotion actually landed is a fact about storage, so the
    // panel re-reads it rather than patching the row optimistically.
    await waitFor(() => expect(adapter.inspectSpan).toHaveBeenCalledTimes(2));
  });

  test('rejecting sends the fact id AND the reviewer', async () => {
    const adapter = adapterWith([fact()]);
    renderRail(adapter, { reviewer: 'Ada Lovelace' });

    fireEvent.click(await screen.findByRole('button', { name: 'Reject' }));
    await waitFor(() => expect(adapter.rejectFact).toHaveBeenCalledWith('fact-1', 'Ada Lovelace'));
  });

  test('the reviewer is trimmed before it is sent, never padded', async () => {
    const adapter = adapterWith([fact()]);
    renderRail(adapter, { reviewer: '  Ada  ' });

    fireEvent.click(await screen.findByRole('button', { name: 'Approve' }));
    await waitFor(() => expect(adapter.approveFact).toHaveBeenCalledWith('fact-1', 'Ada'));
  });

  test('a backend refusal is surfaced, not swallowed', async () => {
    const adapter = adapterWith([fact()], {
      approveFact: vi.fn(async () => {
        throw new Error('reviewer must not be blank');
      }),
    });
    renderRail(adapter, { reviewer: 'Ada' });

    fireEvent.click(await screen.findByRole('button', { name: 'Approve' }));
    expect(await screen.findByText(/reviewer must not be blank/)).toBeTruthy();
  });
});

describe('what the panel refuses to smooth over', () => {
  test('a PROPOSED claim that is IN THE GRAPH shows both truths and why', async () => {
    // There is no overlay GC. An earlier approval committed this claim; the
    // source then changed, resetting the review. Both readings are correct.
    const adapter = adapterWith([fact({ state: 'proposed', inGraph: true })]);
    renderRail(adapter, { reviewer: 'Ada' });

    const claim = await screen.findByTestId('claim-fact-1');
    expect(within(claim).getByText('Proposed')).toBeTruthy();
    expect(within(claim).getByText('in the graph')).toBeTruthy();
    expect(within(claim).getByText(/earlier approval/i)).toBeTruthy();
    // And it is not quietly relabelled as approved.
    expect(within(claim).queryByText('Approved')).toBeNull();
  });

  test('a claim not in the graph says so, rather than saying nothing', async () => {
    const adapter = adapterWith([fact({ state: 'proposed', inGraph: false })]);
    renderRail(adapter, { reviewer: 'Ada' });

    const claim = await screen.findByTestId('claim-fact-1');
    expect(within(claim).getByText('not in the graph')).toBeTruthy();
    expect(within(claim).queryByText(/earlier approval/i)).toBeNull();
  });

  test('a rejection whose source moved invites a deliberate re-review', async () => {
    const adapter = adapterWith([
      fact({
        state: 'rejected',
        reviewer: 'Ada',
        sourceChangedSinceReview: true,
        reviewedContentHash: 'blake3:0000111122223333aaaa',
      }),
    ]);
    renderRail(adapter, { reviewer: 'Ada' });

    const claim = await screen.findByTestId('claim-fact-1');
    expect(within(claim).getByText(/changed since — review it again\?/)).toBeTruthy();
    // Both hashes are shown, so "changed" is checkable rather than asserted.
    expect(within(claim).getByText(/reviewed blake3:00001111…aaaa/)).toBeTruthy();
    expect(within(claim).getByText(/now blake3:9f2c1a4e…ff01/)).toBeTruthy();
    // Two real digests — nothing to explain away.
    expect(within(claim).queryByText(/cannot be compared/)).toBeNull();
  });

  test('an unavailable CURRENT hash is stated — one hash must not read as a comparison', async () => {
    // The engine recorded no digest for the cited text as it now stands. The
    // prompt still asserts the source moved, so the panel has to distinguish
    // "these two differ" from "we could not compare them".
    const adapter = adapterWith([
      fact({
        state: 'rejected',
        reviewer: 'Ada',
        sourceChangedSinceReview: true,
        reviewedContentHash: 'blake3:0000111122223333aaaa',
        provenance: { ...fact().provenance, contentHash: null },
      }),
    ]);
    renderRail(adapter, { reviewer: 'Ada' });

    const claim = await screen.findByTestId('claim-fact-1');
    // The side that IS recorded still stands beside the one that is not.
    expect(within(claim).getByText(/reviewed blake3:00001111…aaaa/)).toBeTruthy();
    expect(within(claim).getByText(/current hash unavailable/)).toBeTruthy();
    // …and the reason, so the absence is an account rather than a shrug.
    expect(
      within(claim).getByText(
        /cannot be compared here — no digest was recorded for the cited text as it now stands/,
      ),
    ).toBeTruthy();
  });

  test('extraction reads as a proposal, with the rule and extractor that made it', async () => {
    const adapter = adapterWith([fact()]);
    renderRail(adapter, { reviewer: 'Ada' });

    const claim = await screen.findByTestId('claim-fact-1');
    expect(within(claim).getByText(/rule D1/)).toBeTruthy();
    expect(within(claim).getByText(/mdreader\.decision@1/)).toBeTruthy();
    expect(within(claim).getByText(/82% confident/)).toBeTruthy();
    expect(within(claim).getByText(/decision_overlay@4/)).toBeTruthy();
  });

  test('a fact with no recorded confidence does not claim zero confidence', async () => {
    const adapter = adapterWith([
      fact({ provenance: { ...fact().provenance, confidence: null } }),
    ]);
    renderRail(adapter, { reviewer: 'Ada' });

    const claim = await screen.findByTestId('claim-fact-1');
    expect(within(claim).getByText(/no confidence recorded/)).toBeTruthy();
    expect(within(claim).queryByText(/0% confident/)).toBeNull();
  });
});

describe('the passage', () => {
  test('a claim’s citation jumps the reader to its cited lines', async () => {
    const onCite = vi.fn();
    const adapter = adapterWith([fact()]);
    renderRail(adapter, { reviewer: 'Ada', onCite });

    fireEvent.click(await screen.findByRole('button', { name: 'notes/adr-3.md:12–18' }));
    expect(onCite).toHaveBeenCalledWith('notes/adr-3.md', 12, 18);
  });

  test('a claim with no located source offers no jump at all', async () => {
    const adapter = adapterWith([
      fact({ provenance: { ...fact().provenance, file: null, startLine: null, endLine: null } }),
    ]);
    renderRail(adapter, { reviewer: 'Ada' });

    const claim = await screen.findByTestId('claim-fact-1');
    expect(within(claim).getByText('no source recorded')).toBeTruthy();
    // No dead affordance: the button simply is not there.
    expect(within(claim).queryByTitle('Go to the cited lines')).toBeNull();
  });

  test('a span with no claims says the passage is prose, and does not fabricate rows', async () => {
    const adapter = adapterWith([]);
    renderRail(adapter, { reviewer: 'Ada' });

    expect(await screen.findByText(/read as prose only/i)).toBeTruthy();
  });

  test('a collapsed rail reads nothing at all', () => {
    const adapter = adapterWith([fact()]);
    renderRail(adapter, { open: false });

    expect(screen.getByTitle('Show the semantic inspector')).toBeTruthy();
    expect(adapter.inspectSpan).not.toHaveBeenCalled();
  });
});
