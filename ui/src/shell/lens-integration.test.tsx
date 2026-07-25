/**
 * Integration tests over the REAL AppShell wiring for the five semantic
 * surfaces, mocking only the IPC boundary (`platform/semantics.js`,
 * `platform/fs.js`), persistence, and the Tauri window.
 *
 * The component tests prove each panel behaves; these prove the panels are
 * actually *reachable* and that the shell threads their results correctly —
 * above all the click-to-source jump, whose value lives entirely in the
 * translation from the engine's repo-relative span to a document the reader
 * opens at exactly those lines.
 *
 * `ReaderPane` is stubbed with a seam that renders what the shell handed it:
 * the pending jump, the lens band, and a reading column with real
 * `data-line-*` blocks. That makes the shell's OWN computation observable —
 * consuming a jump is `MarkdownReader.scrollToLine`'s job and is covered there.
 */

import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type {
  ConstraintReport,
  InspectedFact,
  SchemaDetail,
  SchemaSummary,
  SpanSelection,
} from '../platform/semantics.js';

const h = vi.hoisted(() => ({
  schemas: [] as unknown[],
  report: { schemas_checked: 0, violations: [] } as unknown,
  facts: [] as unknown[],
  inspectCalls: [] as unknown[],
  listSchemas: vi.fn(),
  inspectSpan: vi.fn(),
  checkConstraints: vi.fn(),
}));

vi.mock('../platform/semantics.js', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../platform/semantics.js')>();
  return {
    ...actual,
    makeSemanticsAdapter: () => ({
      listSchemas: h.listSchemas,
      projectCorpus: vi.fn(async () => ({
        schema: 'decision_overlay',
        objects: [],
        morphisms: [],
        truncated: false,
      })),
      describeSchema: vi.fn(async () => DETAIL),
      inspectSpan: h.inspectSpan,
      approveFact: vi.fn(async (factId: string) => ({ factId, promoted: [factId] })),
      rejectFact: vi.fn(async (factId: string) => ({ factId, retracted: [], cascaded: [] })),
      checkConstraints: h.checkConstraints,
      explainQuery: vi.fn(async () => []),
    }),
  };
});

vi.mock('../platform/fs.js', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../platform/fs.js')>();
  return {
    ...actual,
    readDoc: vi.fn(async () => ({ content: '# Note\n', mtime: 0, hash: 'h0' })),
    saveDoc: vi.fn(async () => ({ status: 'saved' as const, newHash: 'h1', mtime: 0 })),
    onFsChanged: vi.fn(async () => () => {}),
    onWatchError: vi.fn(async () => () => {}),
    watchWorkspace: vi.fn(async (root: string) => ({ root, generation: 1 })),
    setWorkspace: vi.fn(async (root: string) => root),
    listWorkspaceFiles: vi.fn(async () => ({
      files: [
        { path: '/ws/notes/services.md', name: 'services.md', dir: '/ws/notes', mtime: 0, size: 0, ignored: false },
        { path: '/ws/note.md', name: 'note.md', dir: '/ws', mtime: 0, size: 0, ignored: false },
      ],
      fromIndex: true,
    })),
    startImport: vi.fn(async () => ({ phase: 'done', filesDone: 0, filesTotal: 0, chunks: 0, error: null })),
    pickWorkspaceFolder: vi.fn(async () => '/ws'),
    resolveWikilink: vi.fn(async () => ({ status: 'unresolved' as const })),
    makeSearchAdapter: vi.fn(() => ({ search: vi.fn(async () => []), related: vi.fn(async () => []) })),
  };
});

vi.mock('./persistence.js', async (importOriginal) => {
  const actual = await importOriginal<typeof import('./persistence.js')>();
  return {
    ...actual,
    isTauri: () => true,
    loadPersisted: vi.fn(async () => ({ session: null, settings: {} })),
    saveSettings: vi.fn(async () => {}),
    sessionWriter: { write: vi.fn(), drain: vi.fn(async () => {}) },
  };
});

vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({
    onCloseRequested: vi.fn(async () => () => {}),
    destroy: vi.fn(async () => {}),
    close: vi.fn(async () => {}),
  }),
}));

// The reading surface's own seam: it renders exactly what the shell computed —
// including BOTH ends of the jump range, so a range narrowed on the way here is
// visible rather than hidden behind the seam. Whether the reader then MARKS that
// whole range is one layer further on, and is covered by `click-to-source.test.tsx`.
vi.mock('./ReaderPane.js', () => ({
  ReaderPane: ({
    jump,
    lensBand,
    onViewAs,
    lensSchema,
  }: {
    jump: { path: string; startLine: number; endLine: number } | null;
    lensBand: React.ReactNode;
    onViewAs?: () => void;
    lensSchema: string | null;
  }) => (
    <div className="reader-region">
      {onViewAs ? (
        <button type="button" onClick={onViewAs}>
          {lensSchema ? `As ${lensSchema}` : 'View as'}
        </button>
      ) : null}
      {lensBand}
      <div data-testid="jump">
        {jump ? `${jump.path}#${jump.startLine}–${jump.endLine}` : 'none'}
      </div>
      <div className="reader-host">
        <p data-line-start="12" data-line-end="18">
          A decision paragraph
        </p>
      </div>
    </div>
  ),
}));

import { readFileSync } from 'node:fs';

import App from '../App';
import { detectMac } from '../keymap/registry';
import { useTabsStore } from '../stores/tabs';
import { useSourcesStore } from '../stores/sources';
import { useSettingsStore } from '../stores/settings';
import { useEditorStore } from '../stores/editor';

const SUMMARY: SchemaSummary = {
  name: 'decision_overlay',
  version: null,
  checksum: null,
  entities: ['Decision'],
  arrowCount: 1,
  attributeCount: 2,
  equationCount: 1,
  approvalRequired: true,
  objectCount: 7,
  proposalCount: 3,
};

const DETAIL: SchemaDetail = {
  name: 'decision_overlay',
  version: null,
  checksum: null,
  typeside: ['String'],
  entities: [{ name: 'Decision', attributes: [] }],
  arrows: [],
  equations: [],
  approvalRequired: true,
};

const VIOLATION_SPAN = {
  file: 'notes/services.md',
  startLine: 41,
  endLine: 47,
  contentHash: 'blake3:cccc3333dddd4444',
};

const REPORT: ConstraintReport = {
  schemasChecked: 2,
  violations: [
    {
      schema: 'architecture_overlay',
      law: 'Dependency.via.exposes = Dependency.provider',
      lhsPath: ['via', 'exposes'],
      rhsPath: ['provider'],
      subject: { id: 'subject-1', entity: 'architecture_overlay.Dependency', source: VIOLATION_SPAN },
      lhs: null,
      rhs: { id: 'right-1', entity: 'architecture_overlay.Component', source: null },
    },
  ],
};

const FACT: InspectedFact = {
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
    file: 'note.md',
    startLine: 12,
    endLine: 18,
    contentHash: 'blake3:aaaa1111bbbb2222',
    schemaVersion: null,
    extractor: 'mdreader.decision@1',
    confidence: 0.7,
    approvedBy: null,
  },
  state: 'proposed',
  reviewer: null,
  inGraph: false,
  reviewedContentHash: null,
  sourceChangedSinceReview: false,
};

const mac = detectMac();
function modKeyDown(key: string, extra: Partial<KeyboardEventInit> = {}) {
  fireEvent.keyDown(window, { key, metaKey: mac, ctrlKey: !mac, ...extra });
}

async function openWorkspace() {
  render(<App />);
  await act(async () => {});
  // The sidebar's picker — the reading pane is stubbed here, so its own
  // empty-state button is not on screen.
  await act(async () => {
    fireEvent.click(screen.getByTitle('Open folder…'));
  });
  await waitFor(() => expect(useSourcesStore.getState().root).toBe('/ws'));
  act(() => {
    useTabsStore.getState().openTab({ path: '/ws/note.md', name: 'note.md' }, { originRoot: '/ws' });
  });
  await waitFor(() => expect(useTabsStore.getState().activeTabId).toBeTruthy());
}

beforeEach(() => {
  h.listSchemas.mockImplementation(async () => [SUMMARY]);
  h.checkConstraints.mockImplementation(async () => REPORT);
  h.inspectSpan.mockImplementation(async (selection: SpanSelection) => {
    h.inspectCalls.push(selection);
    return [FACT];
  });
  h.inspectCalls.length = 0;
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  useTabsStore.getState().reset();
  useSourcesStore.getState().reset();
  useSettingsStore.getState().reset();
  useEditorStore.getState().reset();
});

describe('Constraint Explorer: click-to-source', () => {
  it('jumps the reader to the EXACT file and line range the violation names', async () => {
    await openWorkspace();
    act(() => modKeyDown('L', { shiftKey: true }));

    // The located span, verbatim, is the clickable thing.
    const jumpControl = await screen.findByRole('button', { name: 'notes/services.md:41–47' });
    await act(async () => {
      fireEvent.click(jumpControl);
    });

    // The repo-relative file resolved against the workspace listing…
    await waitFor(() =>
      expect(useTabsStore.getState().tabs.map((tab) => tab.docRef.path)).toContain(
        '/ws/notes/services.md',
      ),
    );
    // …and the reader was handed the violation's OWN lines — the whole range,
    // 41 THROUGH 47, not line 1, not the document's top, and not just 41. This
    // is the assertion the whole surface exists for.
    expect(screen.getByTestId('jump').textContent).toBe('/ws/notes/services.md#41–47');
    // The sheet steps out of the way once it has delivered you somewhere.
    expect(screen.queryByRole('dialog', { name: 'Constraint explorer' })).toBeNull();
  });

  it('opens on ⌘⇧L and reports how many schemas were checked', async () => {
    await openWorkspace();
    act(() => modKeyDown('L', { shiftKey: true }));
    expect(await screen.findByRole('dialog', { name: 'Constraint explorer' })).toBeTruthy();
    expect(screen.getByText(/across 2 schemas/)).toBeTruthy();
  });
});

describe('View As', () => {
  it('opens on ⌘⇧V, adopts a lens, and shows the band over the untouched prose', async () => {
    await openWorkspace();
    act(() => modKeyDown('V', { shiftKey: true }));

    const adopt = await screen.findByRole('button', { name: /Read as this/ });
    await act(async () => {
      fireEvent.click(adopt);
    });
    await waitFor(() => expect(useSettingsStore.getState().lensSchema).toBe('decision_overlay'));

    const band = await screen.findByLabelText('Active lens');
    expect(band.textContent).toContain('decision_overlay');
    expect(band.textContent).toContain('7 in the graph');
    // "3 claims", NOT "3 claims awaiting review": the proposal store counts
    // claims in any review state, so calling them all pending would misreport
    // how much is left to rule on.
    expect(band.textContent).toContain('3 claims');
    expect(band.textContent).not.toContain('awaiting review');
    // The whole point, said out loud on the reading surface.
    expect(band.textContent).toContain('Sources untouched');
    // And no index yet, so no invented version.
    expect(band.textContent).toContain('not yet versioned');
    expect(band.textContent).not.toContain('v0');
  });

  it('returns to the plain reading, and the band goes away entirely', async () => {
    await openWorkspace();
    act(() => useSettingsStore.getState().setLensSchema('decision_overlay'));
    await waitFor(() => expect(screen.queryByLabelText('Active lens')).not.toBeNull());

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Plain reading' }));
    });
    await waitFor(() => expect(screen.queryByLabelText('Active lens')).toBeNull());
  });

  it('offers no View As control at all when no schema is registered', async () => {
    h.listSchemas.mockImplementation(async () => []);
    await openWorkspace();
    await act(async () => {});
    expect(screen.queryByRole('button', { name: 'View as' })).toBeNull();
  });
});

describe('Semantic Inspector', () => {
  it('opens on ⌘⇧I and reads the lines of a real selection in the reading column', async () => {
    await openWorkspace();
    act(() => modKeyDown('I', { shiftKey: true }));
    await waitFor(() => expect(document.querySelector('.lens-rail')).not.toBeNull());

    // Select the paragraph the way a reader would.
    const block = document.querySelector('[data-line-start="12"]')!;
    const selection = window.getSelection()!;
    selection.removeAllRanges();
    const range = document.createRange();
    range.selectNodeContents(block);
    selection.addRange(range);
    await act(async () => {
      fireEvent.mouseUp(document);
    });

    await waitFor(() => expect(h.inspectCalls).toHaveLength(1));
    expect(h.inspectCalls[0]).toEqual({ path: '/ws/note.md', startLine: 12, endLine: 18 });
    expect(await screen.findByText('Decision — Adopt SQLite')).toBeTruthy();
  });

  it('is collapsed by default and shows only a strip', async () => {
    await openWorkspace();
    expect(useSettingsStore.getState().inspectorOpen).toBe(false);
    expect(screen.getByTitle('Show the semantic inspector')).toBeTruthy();
    expect(document.querySelector('.lens-rail')).toBeNull();
  });

  it('sits beside Related as a second column, not on top of it', async () => {
    await openWorkspace();
    act(() => {
      useSettingsStore.getState().setInspectorOpen(true);
      useSettingsStore.getState().setRelatedRailOpen(true);
    });
    await waitFor(() => expect(document.querySelector('.lens-rail')).not.toBeNull());

    const row = document.querySelector('.shell__reader-row')!;
    // Both rails are direct children of the reader row (columns, not overlays),
    // with the apparatus nearest the text.
    expect(document.querySelector('.lens-rail')!.parentElement).toBe(row);
    expect(document.querySelector('.related-rail')!.parentElement).toBe(row);
    const children = [...row.children];
    expect(children.indexOf(document.querySelector('.lens-rail')!)).toBeLessThan(
      children.indexOf(document.querySelector('.related-rail')!),
    );
  });
});

describe('Query Explanation', () => {
  it('opens on ⌘⇧K', async () => {
    await openWorkspace();
    act(() => modKeyDown('K', { shiftKey: true }));
    expect(await screen.findByRole('dialog', { name: 'Query explanation' })).toBeTruthy();
  });
});

describe('focus mode hides the semantic chrome with the rest of it', () => {
  it('applies the focus class while the panels are mounted', async () => {
    await openWorkspace();
    act(() => {
      useSettingsStore.getState().setInspectorOpen(true);
      useSettingsStore.getState().setLensSchema('decision_overlay');
    });
    await waitFor(() => expect(document.querySelector('.lens-rail')).not.toBeNull());

    act(() => modKeyDown('.'));
    await waitFor(() => expect(document.querySelector('.shell--focus')).not.toBeNull());
    // Both live inside the focus scope, so the rule below applies to them.
    expect(document.querySelector('.shell--focus .lens-rail')).not.toBeNull();
    expect(document.querySelector('.shell--focus .lens-band')).not.toBeNull();
  });

  it('the focus-mode rule names every semantic class the shell renders', () => {
    // jsdom does not apply the stylesheet, so the guard is against DRIFT: a
    // class renamed in TSX but not in the focus rule would leave the panel
    // visible in focus mode, and only this catches it.
    const shellCss = readFileSync(`${process.cwd()}/src/shell/shell.css`, 'utf8');
    const hideBlocks: string[] = shellCss.match(/[^{}]+\{\s*display:\s*none;?\s*\}/g) ?? [];
    const focusHide = hideBlocks.find((block) => block.includes('.shell--focus .sidebar'));
    expect(focusHide, 'the focus-mode display:none rule').toBeTruthy();
    for (const cls of ['.lens-rail', '.lens-strip', '.lens-band']) {
      expect(focusHide!).toContain(`.shell--focus ${cls},`);
    }
  });
});
