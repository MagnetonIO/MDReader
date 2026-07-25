/**
 * Dev-only Tauri IPC mock for Playwright end-to-end tests. Uses the official
 * `@tauri-apps/api/mocks` (`mockIPC` + `mockWindows`) to stand in for the Rust
 * command layer with a tiny in-memory corpus, so the e2e suite drives the real
 * shell + real markdown pipeline in a plain browser — no Tauri runtime needed.
 * Loaded only from `e2e.html` / `src/e2e/main.tsx`, never from the production
 * entry, so it can never reach a shipped bundle.
 *
 * The engine commands (`start_import`, `import_status`, `resolve_wikilink`) are
 * mocked at the boundary too: `start_import` reports a finished index directly
 * from its return value (the channel feed is best-effort, so a mock need not
 * drive it), and `resolve_wikilink` answers from a tiny fixed link table.
 */

import { mockIPC, mockWindows } from '@tauri-apps/api/mocks';

const CORPUS: Record<string, string> = {
  '/mock/ws/index.md':
    '# Welcome\n\nA calm place to read.\n\nSee the [[Guide]], a deep link to [[Guide#Section B]], and a [[Nonexistent]] note.\n\n## Getting started\n\nOpen a file from the tree, or press Cmd+P.\n',
  '/mock/ws/docs/guide.md':
    '# Guide\n\nAn introduction.\n\n## Section A\n\nAlpha content.\n\n## Section B\n\nBeta content.\n',
  '/mock/ws/docs/reference.md': '# Reference\n\n## API\n\nReference material.\n',
  // A long document, so split-mode scroll sync has room to move (see the e2e).
  '/mock/ws/long.md':
    '# Long\n\n' +
    Array.from(
      { length: 40 },
      (_unused, index) =>
        `## Section ${index + 1}\n\nParagraph text for section ${index + 1}, with enough words on the line to give the editor and the preview real height to scroll through.\n`,
    ).join('\n'),
};

// A monotonic per-path "disk version" so the mock can produce a NEW hash whenever
// a file's bytes change — enough to exercise the save hash-guard end to end. The
// real backend hashes bytes; the mock's opaque token is equivalent for the UI,
// which only ever compares hashes for equality.
const DISK_VERSION: Record<string, number> = {};

/**
 * Every IPC command the shell actually issued, in order.
 *
 * Recorded so an e2e can assert an **absence** — that an inert control sent
 * nothing — instead of settling for "the button looked disabled". A control that
 * is merely styled as unavailable and a control that cannot submit are different
 * claims, and only a read of what crossed the boundary tells them apart.
 */
const SENT: { cmd: string; args: unknown }[] = [];

/** The current on-disk hash the read/save commands agree on for `path`. */
function diskHash(path: string): string {
  return `h:${path}:${DISK_VERSION[path] ?? 0}`;
}

/** Write `content` to the mock disk and bump its version (⇒ a fresh hash). */
function writeDisk(path: string, content: string): void {
  CORPUS[path] = content;
  DISK_VERSION[path] = (DISK_VERSION[path] ?? 0) + 1;
}

function fileEntries() {
  return Object.keys(CORPUS).map((path) => {
    const name = path.split('/').pop()!;
    return {
      path,
      name,
      dir: path.slice(0, path.length - name.length - 1),
      mtime: 1_700_000_000_000,
      size: CORPUS[path]!.length,
      ignored: false,
    };
  });
}

/** A section-bearing search hit for `path`, aimed at the query's section. */
function searchHit(path: string, query: string) {
  const content = CORPUS[path]!;
  const name = path.split('/').pop()!;
  const lines = content.split('\n');
  const needle = query.toLowerCase();
  let matchIdx = lines.findIndex((line) => line.toLowerCase().includes(needle));
  if (matchIdx < 0) matchIdx = 0;
  // Walk back to the nearest heading — the section the match sits in.
  let headIdx = matchIdx;
  while (headIdx >= 0 && !/^#{1,6}\s/.test(lines[headIdx]!)) headIdx -= 1;
  const startLine = (headIdx >= 0 ? headIdx : 0) + 1;
  const heading = headIdx >= 0 ? lines[headIdx]!.replace(/^#+\s*/, '') : name;
  return {
    chunk_id: `chunk:${path}`,
    document_id: `doc:${path}`,
    file_path: path.replace('/mock/ws/', ''),
    abs_path: path,
    name,
    heading_path: [heading],
    start_line: startLine,
    end_line: startLine + 1,
    score: 1,
    rrf_score: 0.9,
    boosts: { title: 0, path: 0, recency: 0, graph: 0 },
    lex_rank: 1,
    sem_rank: null,
    snippet: `…[${query}]…`,
    mtime: 1_700_000_000_000,
    explain: null,
  };
}

/** The engine's hybrid search, mocked as substring matches over the corpus.
 *  The channel feed is best-effort, so the returned array is authoritative. */
function search(query: string) {
  const q = query.trim().toLowerCase();
  if (q.length === 0) return [];
  return Object.keys(CORPUS)
    .filter((path) => CORPUS[path]!.toLowerCase().includes(q))
    .map((path) => searchHit(path, query));
}

/** related() of a document, mocked with a decomposed unified score. */
function related(docPath: string) {
  return Object.keys(CORPUS)
    .filter((path) => path !== docPath)
    .map((path, index) => ({
      document_id: `doc:${path}`,
      file_path: path.replace('/mock/ws/', ''),
      abs_path: path,
      name: path.split('/').pop()!,
      score: 0.9 - index * 0.2,
      components: { graph: 0.5 - index * 0.1, concept: 0.2, vector: 0.2 - index * 0.1 },
    }));
}

// --------------------------------------------------------------------------
// the semantic surface (P7)
// --------------------------------------------------------------------------
//
// A deliberately AWKWARD fixture: the states these panels exist to render
// honestly are exactly the ones a tidy fixture would never produce. So one
// schema is versioned and one is not; one claim is `proposed` while still
// committed to the graph; one rejection's source has moved since it was ruled
// on; one boost never fires; and one violation's path arrives nowhere.

const DECISION_PROVENANCE = {
  file: 'index.md',
  start_line: 1,
  end_line: 5,
  content_hash: 'blake3:9f2c1a4e88b7d3c2ff01',
  schema_version: '1',
  extractor: 'mdreader.decision@1',
  confidence: 0.82,
  approved_by: null,
};

const SCHEMAS = [
  {
    name: 'decision_overlay',
    version: 1,
    checksum: 'blake3:9f2c1a4e88b7d3c2ff0142ab',
    entities: ['Decision'],
    arrow_count: 1,
    attribute_count: 3,
    equation_count: 1,
    approval_required: true,
    object_count: 2,
    proposal_count: 3,
  },
  {
    // No index has assigned this one a version yet — the panels must say so
    // rather than showing `v0`.
    name: 'architecture_overlay',
    version: null,
    checksum: null,
    entities: ['Component', 'Dependency'],
    arrow_count: 3,
    attribute_count: 2,
    equation_count: 1,
    approval_required: true,
    object_count: 0,
    proposal_count: 0,
  },
];

const SCHEMA_DETAILS: Record<string, unknown> = {
  decision_overlay: {
    name: 'decision_overlay',
    version: 1,
    checksum: 'blake3:9f2c1a4e88b7d3c2ff0142ab',
    typeside: ['String', 'Date'],
    entities: [
      {
        name: 'Decision',
        attributes: [
          { name: 'title', ty: 'String', nullable: false },
          { name: 'status', ty: 'String', nullable: false },
          { name: 'decided_on', ty: 'Date', nullable: true },
        ],
      },
    ],
    arrows: [{ name: 'supersedes', dom: 'Decision', cod: 'Decision' }],
    equations: [
      {
        law: 'Decision.supersedes.supersedes = Decision.supersedes',
        start: 'Decision',
        lhs: ['supersedes', 'supersedes'],
        rhs: ['supersedes'],
      },
    ],
    approval_required: true,
  },
  architecture_overlay: {
    name: 'architecture_overlay',
    version: null,
    checksum: null,
    typeside: ['String'],
    entities: [
      { name: 'Component', attributes: [{ name: 'name', ty: 'String', nullable: false }] },
      { name: 'Dependency', attributes: [{ name: 'kind', ty: 'String', nullable: true }] },
    ],
    arrows: [
      { name: 'via', dom: 'Dependency', cod: 'Component' },
      { name: 'exposes', dom: 'Component', cod: 'Component' },
      { name: 'provider', dom: 'Dependency', cod: 'Component' },
    ],
    equations: [
      {
        law: 'Dependency.via.exposes = Dependency.provider',
        start: 'Dependency',
        lhs: ['via', 'exposes'],
        rhs: ['provider'],
      },
    ],
    approval_required: true,
  },
};

const PROJECTIONS: Record<string, unknown> = {
  decision_overlay: {
    schema: 'decision_overlay',
    objects: [
      {
        id: '11111111-2222-3333-4444-555555555555',
        entity: 'decision_overlay.Decision',
        value: '{"title":"Read markdown, not chrome","status":"accepted"}',
        provenance: DECISION_PROVENANCE,
      },
      {
        id: '66666666-2222-3333-4444-555555555555',
        entity: 'decision_overlay.Decision',
        value: '{"title":"Open files from the tree","status":"accepted"}',
        provenance: { ...DECISION_PROVENANCE, start_line: 7, end_line: 9 },
      },
    ],
    // An edge knows its arrow and its endpoints, and nothing else.
    morphisms: [
      {
        arrow: 'decision_overlay.supersedes',
        source: '66666666-2222-3333-4444-555555555555',
        target: '11111111-2222-3333-4444-555555555555',
      },
    ],
    truncated: false,
  },
  architecture_overlay: {
    schema: 'architecture_overlay',
    objects: [],
    morphisms: [],
    truncated: false,
  },
};

/** The claims derived from a span of `index.md`, one per interesting state. */
const SPAN_FACTS = [
  {
    fact_id: 'fact-proposed',
    kind: 'object',
    schema: 'decision_overlay',
    name: 'decision_overlay.Decision',
    value: '{"title":"Read markdown, not chrome","status":"accepted"}',
    source_id: null,
    target_id: null,
    rule: 'D1',
    extractor: 'mdreader.decision@1',
    provenance: DECISION_PROVENANCE,
    state: 'proposed',
    reviewer: null,
    in_graph: false,
    reviewed_content_hash: null,
    source_changed_since_review: false,
  },
  {
    // Approved once, committed, then the source changed and the review reset.
    // Both readings below are true, and both are rendered.
    fact_id: 'fact-committed-but-proposed',
    kind: 'object',
    schema: 'decision_overlay',
    name: 'decision_overlay.Decision',
    value: '{"title":"Open files from the tree","status":"accepted"}',
    source_id: null,
    target_id: null,
    rule: 'D1',
    extractor: 'mdreader.decision@1',
    provenance: { ...DECISION_PROVENANCE, content_hash: 'blake3:beef0001c0ffee02' },
    state: 'proposed',
    reviewer: 'A. Reader',
    in_graph: true,
    reviewed_content_hash: 'blake3:9f2c1a4e88b7d3c2ff01',
    source_changed_since_review: false,
  },
  {
    // A preserved rejection whose cited text has since moved.
    fact_id: 'fact-rejected-stale',
    kind: 'object',
    schema: 'decision_overlay',
    name: 'decision_overlay.Decision',
    value: '{"title":"Press Cmd+P to jump","status":"draft"}',
    source_id: null,
    target_id: null,
    rule: 'D2',
    extractor: 'mdreader.decision@1',
    provenance: { ...DECISION_PROVENANCE, confidence: null, content_hash: 'blake3:1234abcd5678ef90' },
    state: 'rejected',
    reviewer: 'A. Reader',
    in_graph: false,
    reviewed_content_hash: 'blake3:9f2c1a4e88b7d3c2ff01',
    source_changed_since_review: true,
  },
  {
    fact_id: 'fact-edge',
    kind: 'morphism',
    schema: 'decision_overlay',
    name: 'decision_overlay.supersedes',
    value: null,
    source_id: '66666666-2222-3333-4444-555555555555',
    target_id: '11111111-2222-3333-4444-555555555555',
    rule: 'D3',
    extractor: 'mdreader.decision@1',
    provenance: DECISION_PROVENANCE,
    state: 'proposed',
    reviewer: null,
    in_graph: false,
    reviewed_content_hash: null,
    source_changed_since_review: false,
  },
];

const CONSTRAINT_REPORT = {
  schemas_checked: 2,
  violations: [
    {
      schema: 'architecture_overlay',
      law: 'Dependency.via.exposes = Dependency.provider',
      lhs_path: ['via', 'exposes'],
      rhs_path: ['provider'],
      subject: {
        id: 'aaaaaaaa-2222-3333-4444-555555555555',
        entity: 'architecture_overlay.Dependency',
        source: {
          file: 'docs/guide.md',
          start_line: 5,
          end_line: 7,
          content_hash: 'blake3:cccc3333dddd4444',
        },
      },
      lhs: null, // the composed path arrives nowhere — that IS the failure
      rhs: {
        id: 'bbbbbbbb-2222-3333-4444-555555555555',
        entity: 'architecture_overlay.Component',
        source: {
          file: 'docs/reference.md',
          start_line: 3,
          end_line: 3,
          content_hash: 'blake3:eeee5555ffff6666',
        },
      },
    },
  ],
};

/** `search`, with the full ranking breakdown attached to every hit. */
function explainQuery(query: string) {
  return search(query).map((hit, index) => ({
    ...hit,
    lex_rank: index + 1,
    sem_rank: index === 0 ? 1 : null,
    rrf_score: 0.0325 - index * 0.004,
    score: 0.0421 - index * 0.004,
    boosts: { title: 0.006, path: null, recency: 0.0036, graph: null },
    explain: {
      lex_rank: index + 1,
      sem_rank: index === 0 ? 1 : null,
      rrf_score: 0.0325 - index * 0.004,
      // Two boosts never fired; they are absent, not zero.
      boosts: { title: 0.006, path: null, recency: 0.0036, graph: null },
      final_score: 0.0421 - index * 0.004,
      index_version: 'idx-2026-07-24',
      embedding_model: 'bge-small-en-v1.5',
      provenance: {
        chunk_id: hit.chunk_id,
        section_id: `sect:${hit.file_path}`,
        document_id: hit.document_id,
        file_path: hit.file_path,
        source_id: 'src-77777777-2222-3333',
        source_root: '/mock/ws',
      },
    },
  }));
}

/** The engine's wikilink resolution, mocked from a fixed link table. */
function resolveWikilink(target: string, heading: string | null) {
  if (/^guide$/i.test(target.trim())) {
    // `## Section B` sits on line 9 of guide.md; `## Section A` on line 5.
    const line = heading ? (/section\s*b/i.test(heading) ? 9 : 5) : null;
    return {
      status: 'resolved',
      path: '/mock/ws/docs/guide.md',
      name: 'guide.md',
      line,
      candidates: [],
    };
  }
  return { status: 'unresolved', path: null, name: null, line: null, candidates: [] };
}

/** The window-side test hooks `e2e.html` exposes (dev-only, never shipped). */
export interface E2EHooks {
  /** Simulate an external edit landing on disk (bumps the file's hash). */
  writeDisk: (path: string, content: string) => void;
  /** Every command the shell has issued so far, oldest first. */
  sent: () => { cmd: string; args: unknown }[];
  /** Forget the command log (arm an "and nothing was sent" assertion). */
  clearSent: () => void;
}

export function installTauriMocks(): void {
  // Test hook: let an e2e simulate an EXTERNAL edit landing on disk (bumping the
  // file's hash) so a subsequent save trips the guard — the watcher's own event
  // channel isn't available under a pure IPC mock — and read back what the shell
  // actually sent, so an absence can be asserted rather than assumed.
  (window as unknown as { __E2E__?: E2EHooks }).__E2E__ = {
    writeDisk,
    sent: () => SENT.map((entry) => ({ ...entry })),
    clearSent: () => {
      SENT.length = 0;
    },
  };
  mockWindows('main');
  mockIPC((cmd, args) => {
    SENT.push({ cmd, args });
    switch (cmd) {
      case 'pick_workspace_folder':
        return '/mock/ws';
      case 'set_workspace':
        return (args as { root: string }).root;
      case 'list_workspace_files':
        // Post-import the shell re-lists from the index; the mock corpus stands
        // in for both, flagged as index-backed so the tree renders identically.
        return { entries: fileEntries(), from_index: true };
      case 'start_import':
        // Resolve after a beat so the StatusBar's optimistic "Indexing…" (busy)
        // phase is observable before this final "done" lands — a busy→done
        // transition without the shell needing the (best-effort) channel feed.
        return new Promise((resolve) =>
          setTimeout(
            () =>
              resolve({
                phase: 'done',
                files_done: 3,
                files_total: 3,
                chunks_embedded: 6,
                error: null,
              }),
            400,
          ),
        );
      case 'import_status':
        return { phase: 'idle', files_done: 0, files_total: 0, chunks_embedded: 0, error: null };
      case 'resolve_wikilink': {
        const { target, heading } = args as { target: string; heading: string | null };
        return resolveWikilink(target, heading);
      }
      case 'search':
        return search((args as { query: string }).query);
      case 'related':
        return related((args as { docPath: string }).docPath);

      // --- the semantic surface (P7) -------------------------------------
      case 'list_schemas':
        return SCHEMAS;
      case 'describe_schema':
        return SCHEMA_DETAILS[(args as { schema: string }).schema] ?? null;
      case 'project_corpus':
        return (
          PROJECTIONS[(args as { schema: string }).schema] ?? {
            schema: (args as { schema: string }).schema,
            objects: [],
            morphisms: [],
            truncated: false,
          }
        );
      case 'inspect_span': {
        // Overlap semantics, as the real command has them: a claim is returned
        // when its cited span intersects the selection.
        const { query } = args as { query: { path: string; start_line: number; end_line: number } };
        if (!query.path.endsWith('index.md')) return [];
        return SPAN_FACTS.filter(
          (fact) =>
            fact.provenance.start_line <= query.end_line &&
            fact.provenance.end_line >= query.start_line,
        );
      }
      case 'approve_fact': {
        const { factId, reviewer } = args as { factId: string; reviewer: string };
        // The backend's own refusal, mirrored: a blank reviewer is not a reviewer.
        if (reviewer.trim().length === 0) throw new Error('reviewer must not be blank');
        const fact = SPAN_FACTS.find((candidate) => candidate.fact_id === factId);
        if (fact) {
          fact.state = 'approved';
          fact.reviewer = reviewer;
          fact.in_graph = fact.kind === 'object';
          fact.source_changed_since_review = false;
        }
        return { fact_id: factId, promoted: fact?.kind === 'object' ? [factId] : [] };
      }
      case 'reject_fact': {
        const { factId, reviewer } = args as { factId: string; reviewer: string };
        if (reviewer.trim().length === 0) throw new Error('reviewer must not be blank');
        const fact = SPAN_FACTS.find((candidate) => candidate.fact_id === factId);
        const wasCommitted = fact?.in_graph ?? false;
        if (fact) {
          fact.state = 'rejected';
          fact.reviewer = reviewer;
          fact.in_graph = false;
          fact.source_changed_since_review = false;
        }
        return { fact_id: factId, retracted: wasCommitted ? [factId] : [], cascaded: [] };
      }
      case 'check_constraints':
        return CONSTRAINT_REPORT;
      case 'explain_query':
        return explainQuery((args as { query: string }).query);
      case 'read_doc': {
        const path = (args as { path: string }).path;
        return { content: CORPUS[path] ?? '# Missing\n', mtime: 1_700_000_000_000, hash: diskHash(path) };
      }
      case 'save_doc': {
        // Mirrors the Rust hash guard: unless forced, the write lands only if the
        // buffer's anchor still matches disk; a mismatch is a typed conflict with
        // NOTHING written. On success the bytes are stored and a fresh hash issued.
        const { path, content, expectedHash, force } = args as {
          path: string;
          content: string;
          expectedHash: string;
          force: boolean;
        };
        if (!force && expectedHash !== diskHash(path)) {
          return { status: 'conflict', new_hash: null, mtime: null, current_hash: diskHash(path) };
        }
        writeDisk(path, content);
        return { status: 'saved', new_hash: diskHash(path), mtime: 1_700_000_000_000, current_hash: null };
      }
      case 'watch_workspace':
        // The atomic switch returns the canonical granted root plus the
        // generation its watcher was armed under (mirrors the Rust contract; the
        // UI adopts this as the active workspace and the event-filter reference).
        return { root: (args as { root: string }).root, generation: 1 };
      case 'ping':
        return { echoed: (args as { msg: string }).msg, version: '0.0.0' };
      default:
        // Plugin channels (store, event) — inert, keep the shell from throwing.
        if (cmd.startsWith('plugin:store|load')) return 1;
        if (cmd.startsWith('plugin:store|')) return null;
        if (cmd.startsWith('plugin:event|')) return 0;
        return null;
    }
  });
}
