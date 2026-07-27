/**
 * Semantic view and Schema View.
 *
 * The assertions worth making here are the ones about what the panel says when
 * the engine has *not* told it something: an unbuilt index leaves a schema
 * fully described but unversioned, and a projected relation has no provenance
 * to offer. Both must read as the absences they are.
 */

import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, test, vi } from 'vitest';

import type {
  Projection,
  SchemaDetail,
  SchemaSummary,
  SemanticsAdapter,
} from '../../platform/semantics.js';
import { LensSheet } from './LensSheet.js';

afterEach(cleanup);

function summary(overrides: Partial<SchemaSummary> = {}): SchemaSummary {
  return {
    name: 'decision_overlay',
    version: 4,
    checksum: 'blake3:9f2c1a4e88b7d3c2ff01',
    entities: ['Decision'],
    arrowCount: 2,
    attributeCount: 3,
    equationCount: 1,
    approvalRequired: true,
    objectCount: 12,
    proposalCount: 5,
    ...overrides,
  };
}

function detail(overrides: Partial<SchemaDetail> = {}): SchemaDetail {
  return {
    name: 'decision_overlay',
    version: 4,
    checksum: 'blake3:9f2c1a4e88b7d3c2ff01',
    typeside: ['String', 'Date'],
    entities: [
      {
        name: 'Decision',
        attributes: [
          { name: 'title', ty: 'String', nullable: false },
          { name: 'decided_on', ty: 'Date', nullable: true },
        ],
      },
      { name: 'Component', attributes: [] },
    ],
    arrows: [{ name: 'affects', dom: 'Decision', cod: 'Component' }],
    equations: [
      {
        law: 'Dependency.via.exposes = Dependency.provider',
        start: 'Dependency',
        lhs: ['via', 'exposes'],
        rhs: ['provider'],
      },
    ],
    approvalRequired: true,
    ...overrides,
  };
}

function projection(overrides: Partial<Projection> = {}): Projection {
  return {
    schema: 'decision_overlay',
    objects: [
      {
        id: 'obj-1',
        entity: 'decision_overlay.Decision',
        value: '{"title":"Adopt SQLite"}',
        provenance: {
          file: 'notes/adr-3.md',
          startLine: 12,
          endLine: 18,
          contentHash: 'blake3:aaaa1111bbbb2222',
          schemaVersion: '4',
          extractor: 'mdreader.decision@1',
          confidence: 0.9,
          approvedBy: 'Ada',
        },
      },
    ],
    morphisms: [{ arrow: 'decision_overlay.affects', source: 'obj-1', target: 'obj-2' }],
    truncated: false,
    ...overrides,
  };
}

function adapterWith(overrides: Partial<SemanticsAdapter> = {}): SemanticsAdapter {
  return {
    listSchemas: vi.fn(async () => [summary()]),
    projectCorpus: vi.fn(async () => projection()),
    describeSchema: vi.fn(async () => detail()),
    inspectSpan: vi.fn(async () => []),
    approveFact: vi.fn(),
    rejectFact: vi.fn(),
    checkConstraints: vi.fn(),
    explainQuery: vi.fn(),
    queryFederatedDocument: vi.fn(),
    cancelFederatedDocument: vi.fn(async () => {}),
    ...overrides,
  } as SemanticsAdapter;
}

function renderSheet(
  adapter: SemanticsAdapter,
  props: Partial<React.ComponentProps<typeof LensSheet>> = {},
) {
  return render(
    <LensSheet
      adapter={adapter}
      activeSchema={null}
      onChoose={vi.fn()}
      onCite={vi.fn()}
      onClose={vi.fn()}
      {...props}
    />,
  );
}

describe('the thesis, stated', () => {
  test('names this a semantic interpretation, not another reader layout', async () => {
    renderSheet(adapterWith());
    expect(await screen.findByRole('dialog', { name: 'Semantic view' })).toBeTruthy();
    expect(screen.getByText(/changes CatDB's interpretation/i)).toBeTruthy();
    expect(screen.getByText(/does not change the Markdown file or its format/i)).toBeTruthy();
  });

  test('says the corpus is reinterpreted and the files are not touched', async () => {
    renderSheet(adapterWith());
    expect(await screen.findByText(/files on disk are never touched/i)).toBeTruthy();
  });

  test('choosing a lens reports the schema name, and the plain reading is always offered', async () => {
    const onChoose = vi.fn();
    renderSheet(adapterWith(), { onChoose });

    fireEvent.click(await screen.findByRole('button', { name: /Read as this/ }));
    expect(onChoose).toHaveBeenCalledWith('decision_overlay');

    fireEvent.click(screen.getByRole('button', { name: /Plain reading/ }));
    expect(onChoose).toHaveBeenCalledWith(null);
  });
});

describe('a schema with no index behind it', () => {
  test('reads as "not yet versioned" — not v0, not blank — and says why', async () => {
    const adapter = adapterWith({
      listSchemas: vi.fn(async () => [summary({ version: null, checksum: null, objectCount: 0 })]),
      describeSchema: vi.fn(async () => detail({ version: null, checksum: null })),
    });
    renderSheet(adapter);

    expect(await screen.findByText('not yet versioned')).toBeTruthy();
    expect(screen.getByText('no checksum yet')).toBeTruthy();
    expect(screen.queryByText('v0')).toBeNull();
    // The reason the catalog can be empty while the shape is complete.
    expect(screen.getByText(/Versions and checksums are assigned when the index is built/)).toBeTruthy();
  });

  test('still describes the shape in full, because registration already knows it', async () => {
    const adapter = adapterWith({
      listSchemas: vi.fn(async () => [summary({ version: null, checksum: null })]),
      describeSchema: vi.fn(async () => detail({ version: null, checksum: null })),
    });
    renderSheet(adapter);

    // The registration reading is intact even with no catalog entry.
    expect(
      await screen.findByText('1 entity · 3 attributes · 2 arrows · 1 law · approval required'),
    ).toBeTruthy();
    // Entities, their attributes, and the arrows all describe fine unversioned.
    await waitFor(() => expect(screen.getAllByText('Decision').length).toBeGreaterThan(0));
    expect(screen.getByText('title')).toBeTruthy();
    expect(screen.getByText('affects')).toBeTruthy();
  });

  test('a versioned schema shows its real version', async () => {
    renderSheet(adapterWith());
    expect(await screen.findByText('v4')).toBeTruthy();
    expect(screen.queryByText('not yet versioned')).toBeNull();
  });
});

describe('the shape', () => {
  test('a law is said in words, drawn as a chain, AND kept verbatim', async () => {
    renderSheet(adapterWith());

    expect(
      await screen.findByText(
        'Starting from any Dependency, following via, then exposes must land on the same object as following provider.',
      ),
    ).toBeTruthy();
    expect(screen.getByText('Dependency → via → exposes')).toBeTruthy();
    // The kernel's own rendering survives the translation.
    expect(screen.getByText('Dependency.via.exposes = Dependency.provider')).toBeTruthy();
  });

  test('an entity with no attributes says so instead of rendering an empty list', async () => {
    renderSheet(adapterWith());
    expect(await screen.findByText('no attributes')).toBeTruthy();
  });

  test('an optional attribute is marked optional', async () => {
    renderSheet(adapterWith());
    expect(await screen.findByText('optional')).toBeTruthy();
  });
});

describe('the contents', () => {
  test('a projected object cites its source and jumps there', async () => {
    const onCite = vi.fn();
    renderSheet(adapterWith(), { onCite });

    fireEvent.click(await screen.findByRole('tab', { name: 'Contents' }));
    const cite = await screen.findByRole('button', { name: 'notes/adr-3.md:12–18' });
    fireEvent.click(cite);
    expect(onCite).toHaveBeenCalledWith('notes/adr-3.md', 12, 18);
  });

  test('a projected RELATION offers no citation, and the panel says where one lives', async () => {
    // Storage returns only (arrow, source, target) for an edge. Rendering a
    // dead provenance control would be worse than explaining the absence.
    renderSheet(adapterWith());

    fireEvent.click(await screen.findByRole('tab', { name: 'Contents' }));
    expect(
      await screen.findByText(/Projected relations carry no citation/),
    ).toBeTruthy();
    expect(screen.getByText(/Select the passage in the reader/)).toBeTruthy();
    // Exactly one citation button on the pane: the object's. The edge has none.
    expect(screen.getAllByTitle('Go to the cited lines')).toHaveLength(1);
  });

  test('a truncated projection says it is showing only part of the whole', async () => {
    const adapter = adapterWith({
      projectCorpus: vi.fn(async () => projection({ truncated: true })),
    });
    renderSheet(adapter);

    fireEvent.click(await screen.findByRole('tab', { name: 'Contents' }));
    expect(await screen.findByText(/first 1 object, 1 relation/)).toBeTruthy();
    expect(screen.getByText(/holds more than is shown here/)).toBeTruthy();
  });

  test('an empty projection invites review rather than reading as an error', async () => {
    const adapter = adapterWith({
      projectCorpus: vi.fn(async () => projection({ objects: [], morphisms: [] })),
    });
    renderSheet(adapter);

    fireEvent.click(await screen.findByRole('tab', { name: 'Contents' }));
    expect(await screen.findByText(/Approve claims in the Inspector/)).toBeTruthy();
  });
});
