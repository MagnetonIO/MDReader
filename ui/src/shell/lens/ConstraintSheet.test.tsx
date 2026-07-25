/**
 * The Constraint Explorer.
 *
 * The one thing this surface must never do is lose a violation's span. The
 * engine located it — schema, law, both paths, the offending object, and the
 * exact markdown lines — and a panel that degrades that to "something is
 * inconsistent" has thrown away the only part a person can act on. So the
 * jump is asserted to carry the located span through **whole**.
 */

import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, test, vi } from 'vitest';

import type { ConstraintReport, SemanticsAdapter, Violation } from '../../platform/semantics.js';
import { ConstraintSheet } from './ConstraintSheet.js';

afterEach(cleanup);

const SUBJECT_SPAN = {
  file: 'notes/services.md',
  startLine: 41,
  endLine: 47,
  contentHash: 'blake3:cccc3333dddd4444',
};

function violation(overrides: Partial<Violation> = {}): Violation {
  return {
    schema: 'architecture_overlay',
    law: 'Dependency.via.exposes = Dependency.provider',
    lhsPath: ['via', 'exposes'],
    rhsPath: ['provider'],
    subject: {
      id: 'subject-1',
      entity: 'architecture_overlay.Dependency',
      source: SUBJECT_SPAN,
    },
    lhs: {
      id: 'left-1',
      entity: 'architecture_overlay.Component',
      source: { file: 'notes/a.md', startLine: 3, endLine: 3, contentHash: null },
    },
    rhs: {
      id: 'right-1',
      entity: 'architecture_overlay.Component',
      source: { file: 'notes/b.md', startLine: 9, endLine: 11, contentHash: null },
    },
    ...overrides,
  };
}

function adapterWith(report: ConstraintReport): SemanticsAdapter {
  return {
    listSchemas: vi.fn(),
    projectCorpus: vi.fn(),
    describeSchema: vi.fn(),
    inspectSpan: vi.fn(),
    approveFact: vi.fn(),
    rejectFact: vi.fn(),
    checkConstraints: vi.fn(async () => report),
    explainQuery: vi.fn(),
  } as SemanticsAdapter;
}

function renderSheet(adapter: SemanticsAdapter, onJump = vi.fn()) {
  render(<ConstraintSheet adapter={adapter} onJump={onJump} onClose={vi.fn()} />);
  return onJump;
}

describe('click-to-source', () => {
  test('a violation hands over its located span WHOLE — file, both lines, hash', async () => {
    const onJump = renderSheet(
      adapterWith({ schemasChecked: 2, violations: [violation()] }),
    );

    fireEvent.click(await screen.findByRole('button', { name: 'notes/services.md:41–47' }));

    // Not a document link, not a rounded line, not a summary: the exact span.
    expect(onJump).toHaveBeenCalledWith(SUBJECT_SPAN);
    expect(onJump.mock.calls[0]![0]).toMatchObject({ startLine: 41, endLine: 47 });
  });

  test('every located object on a violation is independently reachable', async () => {
    // The subject AND wherever each path landed — a disagreement is only
    // diagnosable if you can read all three places.
    const onJump = renderSheet(adapterWith({ schemasChecked: 1, violations: [violation()] }));

    fireEvent.click(await screen.findByRole('button', { name: 'notes/a.md:3' }));
    expect(onJump).toHaveBeenLastCalledWith(
      expect.objectContaining({ file: 'notes/a.md', startLine: 3 }),
    );

    fireEvent.click(screen.getByRole('button', { name: 'notes/b.md:9–11' }));
    expect(onJump).toHaveBeenLastCalledWith(
      expect.objectContaining({ file: 'notes/b.md', startLine: 9, endLine: 11 }),
    );
  });

  test('an object with no located span offers no jump rather than a jump to nowhere', async () => {
    const onJump = renderSheet(
      adapterWith({
        schemasChecked: 1,
        violations: [
          violation({
            subject: { id: 'subject-1', entity: 'architecture_overlay.Dependency', source: null },
          }),
        ],
      }),
    );

    await waitFor(() => expect(screen.getAllByText('no source recorded').length).toBeGreaterThan(0));
    // The subject's own span is absent, so only the two landing objects jump —
    // and no third, disabled control stands in for the one that cannot.
    expect(screen.getAllByTitle('Open the document at these lines')).toHaveLength(2);
    expect(onJump).not.toHaveBeenCalled();
  });
});

describe('a violation, made legible', () => {
  test('states the law in words, as a chain, and verbatim', async () => {
    renderSheet(adapterWith({ schemasChecked: 1, violations: [violation()] }));

    expect(
      await screen.findByText(
        'Starting from any Dependency, following via, then exposes must land on the same object as following provider.',
      ),
    ).toBeTruthy();
    expect(screen.getByText('Dependency → via → exposes')).toBeTruthy();
    expect(screen.getByText('Dependency.via.exposes = Dependency.provider')).toBeTruthy();
  });

  test('says what actually went wrong', async () => {
    renderSheet(adapterWith({ schemasChecked: 1, violations: [violation()] }));
    expect(await screen.findByText('Both paths arrive, but at different objects.')).toBeTruthy();
  });

  test('a missing arrow is named as the failure, not hidden as an empty cell', async () => {
    renderSheet(
      adapterWith({ schemasChecked: 1, violations: [violation({ lhs: null })] }),
    );

    expect(
      await screen.findByText('Following via, then exposes arrives nowhere: an arrow on that side is missing.'),
    ).toBeTruthy();
    expect(screen.getByText('nowhere — the arrow is missing')).toBeTruthy();
  });

  test('a clean run says every law holds, and how many schemas were checked', async () => {
    renderSheet(adapterWith({ schemasChecked: 3, violations: [] }));
    expect(await screen.findByText('Every law holds across 3 schemas.')).toBeTruthy();
  });
});
