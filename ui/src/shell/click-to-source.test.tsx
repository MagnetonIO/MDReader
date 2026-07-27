/**
 * Click-to-source, end to end: from the affordance inside a semantic surface all
 * the way into the rendered reading column.
 *
 * Every other test of this path stops at a seam — `ConstraintSheet.test.tsx`
 * stops at the sheet's `onJump`, `lens-integration.test.tsx` stops at the props
 * the shell hands `ReaderPane`. Both were green while a violation citing lines
 * 41–47 was landing the reader on line 41 with nothing to say it reached 47,
 * because the range was being dropped one layer PAST where they look. So this
 * file stubs nothing below the IPC boundary: the real `ReaderPane`, the real
 * markdown pipeline and the real `MarkdownReader` all render, and the assertion
 * is on the lines the reader actually MARKED.
 *
 * The marking mechanism is the app's existing line-range provenance contract:
 * every top-level block is stamped with `[data-line-start, data-line-end]`, and
 * the reader marks each block whose stamp overlaps the cited range. So "the
 * whole extent is marked" is checkable in stamped source lines, not in pixels.
 */

import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type { ConstraintReport, InspectedFact, SchemaSummary } from '../platform/semantics.js';

const h = vi.hoisted(() => ({
  listSchemas: vi.fn(),
  inspectSpan: vi.fn(),
  checkConstraints: vi.fn(),
  disk: new Map<string, string>(),
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
      describeSchema: vi.fn(),
      inspectSpan: h.inspectSpan,
      approveFact: vi.fn(),
      rejectFact: vi.fn(),
      checkConstraints: h.checkConstraints,
      explainQuery: vi.fn(async () => []),
    }),
  };
});

vi.mock('../platform/fs.js', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../platform/fs.js')>();
  return {
    ...actual,
    readDoc: vi.fn(async (path: string) => ({
      content: h.disk.get(path) ?? '# missing\n',
      mtime: 0,
      hash: 'h0',
    })),
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

vi.mock('../platform/settings.js', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../platform/settings.js')>();
  const source = {
    id: 'e'.repeat(64),
    name: 'Workspace',
    path: '/ws',
    state: { kind: 'available' as const },
    index_state: 'present' as const,
    active: false,
    restore_candidate: false,
  };
  return {
    ...actual,
    pickAndAddLibrarySource: vi.fn(async () => ({ source, added: true })),
    watchLibrarySource: vi.fn(async () => ({ root: source.path, generation: 1 })),
    grantLibrarySource: vi.fn(async () => source.path),
    markLibrarySourceActive: vi.fn(async () => ({
      version: 4,
      sources: [{ ...source, active: true }],
      cleanup: { kind: 'complete' as const },
    })),
    recordDiagnosticEventNonBlocking: vi.fn(),
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

import App from '../App';
import { detectMac } from '../keymap/registry';
import { useTabsStore } from '../stores/tabs';
import { useSourcesStore } from '../stores/sources';
import { useSettingsStore } from '../stores/settings';
import { useEditorStore } from '../stores/editor';

/**
 * A note whose Nth paragraph sits on source line 2N−1 (one line each, blank line
 * between). That makes every assertion below nameable in SOURCE LINES — the same
 * coordinates the engine cites in — rather than in block indices. The `label`
 * makes the two documents tellable apart, so "the right lines" can be checked
 * against "of the right file".
 */
function numberedNote(label: string, paragraphs: number): string {
  return `${Array.from({ length: paragraphs }, (_, i) => `${label} paragraph ${i + 1}.`).join('\n\n')}\n`;
}

const SERVICES = '/ws/notes/services.md';
const NOTE = '/ws/note.md';

/** The violation the Constraint Explorer lists: located at lines 41 THROUGH 47. */
const VIOLATION_SPAN = {
  file: 'notes/services.md',
  startLine: 41,
  endLine: 47,
  contentHash: 'blake3:cccc3333dddd4444',
};

const REPORT: ConstraintReport = {
  schemasChecked: 1,
  violations: [
    {
      schema: 'architecture_overlay',
      law: 'Dependency.via.exposes = Dependency.provider',
      lhsPath: ['via', 'exposes'],
      rhsPath: ['provider'],
      subject: { id: 'subject-1', entity: 'architecture_overlay.Dependency', source: VIOLATION_SPAN },
      lhs: null,
      rhs: null,
    },
  ],
};

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

/** A claim the Inspector lists, cited at lines 13 THROUGH 17 of the open note. */
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
    startLine: 13,
    endLine: 17,
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

/**
 * The source lines the reader has MARKED, read back off the blocks' own stamps.
 * A marked block carries the pulse background; an unmarked one carries no inline
 * style at all, so this reports what the reading column is actually showing.
 */
function markedBlocks(): { lines: number[]; text: string }[] {
  const marked: { lines: number[]; text: string }[] = [];
  for (const block of document.querySelectorAll<HTMLElement>('.reader-host .mdreader-block')) {
    if (block.style.background === '') continue;
    const stamped = block.matches('[data-line-start]')
      ? block
      : block.querySelector<HTMLElement>('[data-line-start]');
    if (!stamped) continue;
    const start = Number(stamped.getAttribute('data-line-start'));
    const end = Number(stamped.getAttribute('data-line-end'));
    const lines: number[] = [];
    for (let line = start; line <= end; line += 1) lines.push(line);
    marked.push({ lines, text: block.textContent ?? '' });
  }
  return marked;
}

function markedLines(): number[] {
  return markedBlocks().flatMap((block) => block.lines);
}

async function openWorkspace() {
  render(<App />);
  await act(async () => {});
  // The sidebar's picker (the reader's own empty-state button carries no title).
  await act(async () => {
    fireEvent.click(screen.getByTitle('Open folder…'));
  });
  await waitFor(() => expect(useSourcesStore.getState().root).toBe('/ws'));
  act(() => {
    useTabsStore.getState().openTab({ path: NOTE, name: 'note.md' }, { originRoot: '/ws' });
  });
  await waitFor(() => expect(document.querySelector('.reader-host .mdreader-block')).not.toBeNull());
}

beforeEach(() => {
  // jsdom has no layout, so scrolling is a no-op — the landing is asserted
  // through the marked range, which is a fact about the DOM.
  Element.prototype.scrollIntoView = vi.fn();
  h.disk.set(SERVICES, numberedNote('Services', 30));
  h.disk.set(NOTE, numberedNote('Note', 30));
  h.listSchemas.mockImplementation(async () => [SUMMARY]);
  h.checkConstraints.mockImplementation(async () => REPORT);
  h.inspectSpan.mockImplementation(async () => [FACT]);
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  h.disk.clear();
  useTabsStore.getState().reset();
  useSourcesStore.getState().reset();
  useSettingsStore.getState().reset();
  useEditorStore.getState().reset();
});

describe('a located violation lands on the WHOLE range, not just its first line', () => {
  it('marks lines 41 through 47 in the reader — 47 survives the trip from the sheet', async () => {
    await openWorkspace();
    act(() => modKeyDown('L', { shiftKey: true }));

    // The located span, verbatim, is the clickable thing.
    const jumpControl = await screen.findByRole('button', { name: 'notes/services.md:41–47' });
    await act(async () => {
      fireEvent.click(jumpControl);
    });

    // The repo-relative file resolved, and the cited document is the one open.
    await waitFor(() =>
      expect(useTabsStore.getState().tabs.map((tab) => tab.docRef.path)).toContain(SERVICES),
    );
    await waitFor(() => expect(markedLines().length).toBeGreaterThan(0));

    const marked = markedLines();
    // The FIRST line — the landing, which was already right.
    expect(marked).toContain(41);
    // The EXTENT — every line the violation named, up to and including the last.
    // This is what a startLine-only jump silently loses: 43, 45 and 47 go
    // unmarked and the reader is left believing the violation is about line 41.
    expect(marked).toContain(43);
    expect(marked).toContain(45);
    expect(marked).toContain(47);
    // And no further: the range is carried whole, not widened to be safe.
    expect(marked).not.toContain(39);
    expect(marked).not.toContain(49);
    // The right lines OF THE RIGHT FILE — the jump crossed documents, and the
    // marks are in the one the violation named, not in the one being read.
    for (const block of markedBlocks()) expect(block.text).toContain('Services paragraph');
  });
});

describe('a claim’s citation lands on the whole cited passage too', () => {
  it('marks lines 13 through 17 — the Inspector is not a first-line-only jump', async () => {
    await openWorkspace();
    act(() => modKeyDown('I', { shiftKey: true }));
    await waitFor(() => expect(document.querySelector('.lens-rail')).not.toBeNull());

    // Select a passage the way a reader would, so the rail has claims to show.
    const block = document.querySelector('.reader-host [data-line-start="5"]')!;
    const selection = window.getSelection()!;
    selection.removeAllRanges();
    const range = document.createRange();
    range.selectNodeContents(block);
    selection.addRange(range);
    await act(async () => {
      fireEvent.mouseUp(document);
    });

    const citation = await screen.findByRole('button', { name: 'note.md:13–17' });
    await act(async () => {
      fireEvent.click(citation);
    });

    await waitFor(() => expect(markedLines().length).toBeGreaterThan(0));
    const marked = markedLines();
    expect(marked).toContain(13);
    expect(marked).toContain(15);
    expect(marked).toContain(17);
    expect(marked).not.toContain(11);
    expect(marked).not.toContain(19);
  });
});
