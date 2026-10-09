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
 *
 * Loading the page with `?nomodel` stands the whole backend in the state the
 * shipping app reaches when the embedding model cannot be loaded: `start_import`
 * settles on `model-unavailable`, the listing comes off the filesystem rather
 * than an index, `search` still answers but marks its hits lexical-only, and
 * `explain_query` refuses. See {@link MODEL_UNAVAILABLE}.
 *
 * Loading it with `?modelloading` stands in for the state BEFORE that verdict —
 * the model is still being fetched/deserialized, so `start_import` has not
 * answered and will not for as long as the load takes. See
 * {@link MODEL_LOADING}.
 *
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

const MOCK_LIBRARY_ID = '7'.repeat(64);
let mockLibraryAdded = false;
let mockLibraryActive = false;
let mockDiagnosticLevel: 'errors' | 'standard' | 'verbose' = 'standard';
function mockLibrarySource() {
  return {
    id: MOCK_LIBRARY_ID,
    name: 'ws',
    path: '/mock/ws',
    state: { kind: 'available' },
    index_state: mockLibraryActive && !MODEL_UNAVAILABLE ? 'present' : 'not-created',
    active: mockLibraryActive,
    restore_candidate: mockLibraryActive,
  };
}

function mockLibrarySnapshot() {
  return {
    version: 4,
    sources: mockLibraryAdded ? [mockLibrarySource()] : [],
    cleanup: { kind: 'complete' },
  };
}

function mockDiagnosticInfo() {
  return {
    level: mockDiagnosticLevel,
    max_file_bytes: 256 * 1024,
    max_retained_files: 3,
    max_export_bytes: 512 * 1024,
  };
}

/**
 * Whether this run stands in for a shipping app whose embedding model cannot be
 * loaded (no cached weights, no network). Armed from the page URL *before* the
 * shell mounts, because the workspace restore happens on mount — the failure has
 * to be in place for the very first `start_import`.
 *
 * The point of driving it here rather than mocking one command in the spec: the
 * whole backend contract moves together (import phase, search degradation), so
 * the spec asserts the app's behaviour under a coherent backend state instead of
 * a hand-held one.
 */
const MODEL_UNAVAILABLE =
  typeof window !== 'undefined' && new URLSearchParams(window.location.search).has('nomodel');

/**
 * Whether this run stands in for a shipping app whose embedding model is STILL
 * LOADING — the state every launch passes through, and the one a first run on a
 * slow link (or a stalled download) sits in for the whole 120s deadline.
 *
 * Distinct from {@link MODEL_UNAVAILABLE}, which is the *settled* failure: here
 * `start_import` never resolves at all. That is what the real backend does while
 * `ensure_model` is awaiting the weights, and it is the precise condition under
 * which the app was reported blank — so the spec drives it to prove the shell,
 * the tree and reading do not wait on it.
 */
const MODEL_LOADING =
  typeof window !== 'undefined' && new URLSearchParams(window.location.search).has('modelloading');

/**
 * Whether this run stands in for a workspace whose folder the OS will not let
 * the app READ — on macOS, a protected folder (`~/Documents`, `~/Desktop`,
 * `~/Downloads`) opened by an app that holds no privacy grant, where every
 * `opendir` comes back `EPERM`.
 *
 * This is the shipped bug's exact condition, and the reason it was invisible: a
 * terminal launch inherits the terminal's grants and lists fine, so only the
 * bundled `.app` ever hits it. The backend now answers with the typed
 * `ListingError::Unreadable`, and the spec drives this flag to prove the shell
 * renders that as an unreadable folder — NOT as a folder with no markdown.
 */
const UNREADABLE_FOLDER =
  typeof window !== 'undefined' && new URLSearchParams(window.location.search).has('unreadable');

/**
 * Whether this run stands in for a workspace folder that is perfectly readable
 * and genuinely holds no markdown. The control for {@link UNREADABLE_FOLDER}:
 * the empty state must survive the fix, so the two outcomes are asserted apart.
 */
const EMPTY_FOLDER =
  typeof window !== 'undefined' && new URLSearchParams(window.location.search).has('emptyfolder');

/**
 * Whether this run stands in for a listing that has NOT answered yet — the
 * window in which macOS is holding its privacy prompt open, because the
 * `opendir` behind the listing blocks until the user answers. Observed live in
 * the shipping `.app`: the sidebar claimed "No markdown files in this folder
 * yet." while the consent sheet was still on screen and nothing had been read.
 */
const LISTING_PENDING =
  typeof window !== 'undefined' &&
  new URLSearchParams(window.location.search).has('listingpending');

/** The OS's own words when macOS's privacy layer withholds a folder. */
const UNREADABLE_REASON =
  'IO error for operation on /mock/ws: Operation not permitted (os error 1)';

/** The remedy the backend ships for macOS — a System Settings grant. */
const UNREADABLE_REMEDY =
  'macOS is withholding this folder from MDReader. Open System Settings › Privacy & Security › Files and Folders, enable the folder for MDReader (or grant Full Disk Access), then choose the folder again.';

/** The reason the (mocked) backend observed — the real one is a load timeout. */
const MODEL_FAILURE_REASON = 'timed out after 120s (the first run downloads the model)';

/** The in-flight `loading` status: the model is being fetched/deserialized. */
function modelLoadingProgress() {
  return {
    phase: 'loading',
    files_done: 0,
    files_total: 0,
    chunks_embedded: 0,
    error: null,
  };
}

/** The `model-unavailable` import status: no files indexed, and a stated reason. */
function modelUnavailableProgress() {
  return {
    phase: 'model-unavailable',
    files_done: 0,
    files_total: 0,
    chunks_embedded: 0,
    error: `the embedding model could not be loaded — ${MODEL_FAILURE_REASON}`,
  };
}

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
 *  The channel feed is best-effort, so the returned value is authoritative. */
function searchHits(query: string) {
  const q = query.trim().toLowerCase();
  if (q.length === 0) return [];
  return Object.keys(CORPUS)
    .filter((path) => CORPUS[path]!.toLowerCase().includes(q))
    .map((path) => searchHit(path, query));
}

/**
 * `search`'s wire shape: the hits plus whether the semantic arm actually ran.
 * Mirrors the Rust `SearchResults` — the lexical arm needs no embedder, so with
 * the model unavailable the hits still come back, labelled as lexical-only.
 */
function search(query: string) {
  return {
    hits: searchHits(query),
    semantic_unavailable:
      MODEL_UNAVAILABLE || MODEL_LOADING
        ? `the embedding model could not be loaded — ${MODEL_FAILURE_REASON}`
        : null,
  };
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

/**
 * Browser-only fixture for the P8 product surface. It mirrors the generated
 * Tauri DTO exactly; the native leg is the proof that these values are produced
 * by the filesystem, Git, and frontmatter adapters rather than this fixture.
 */
function federatedDocument(path: string) {
  const relative = path.replace(/^\/mock\/ws\/?/, '');
  return {
    query_id: '00000000-0000-4000-8000-000000000008',
    schema: {
      name: 'document',
      version: 2,
      checksum: 'blake3:federated-document-fixture',
      entities: ['Document'],
      arrow_count: 0,
      attribute_count: 6,
      equation_count: 0,
      approval_required: false,
      object_count: 0,
      proposal_count: 0,
    },
    completeness: 'complete',
    document: {
      path: relative,
      fields: [
        {
          name: 'title',
          value: 'Reference',
          winner: 'frontmatter',
          explanation: '`title` answered by `frontmatter.title`',
          providers: [
            {
              source: 'frontmatter',
              attribute: 'title',
              verdict: 'observed',
              value: 'Reference',
              reason: null,
              declared_fallback: ['YAML `title:`', 'first H1'],
            },
          ],
        },
        {
          name: 'modifiedAt',
          value: 1700000000000,
          winner: 'filesystem',
          explanation:
            '`modifiedAt` answered by `filesystem.modifiedAt`; `git.modifiedAt` also observed a value',
          providers: [
            {
              source: 'filesystem',
              attribute: 'modifiedAt',
              verdict: 'observed',
              value: 1700000000000,
              reason: null,
              declared_fallback: [],
            },
            {
              source: 'git',
              attribute: 'modifiedAt',
              verdict: 'observed',
              value: 1690000000000,
              reason: null,
              declared_fallback: [],
            },
          ],
        },
      ],
    },
    sources: ['filesystem', 'git', 'frontmatter'].map((source) => ({
      source,
      outcome: 'ran',
      execution: 'virtual',
      freshness: 'fresh',
      age_ms: 0,
      scanned: 1,
      returned: 1,
      pushed: ['path'],
      residual: [],
      refusal: null,
    })),
  };
}

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
  return searchHits(query).map((hit, index) => ({
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
  // `App` selects the desktop/mobile composition through the OS plugin's
  // compile-time platform record. `mockIPC` and `mockWindows` intentionally do
  // not manufacture plugin globals, so install the one value the shipping
  // composition reads before React renders. Without it `isTauri()` is true but
  // `platform()` dereferences an absent `__TAURI_OS_PLUGIN_INTERNALS__`, leaving
  // the E2E page blank before any behavioural assertion can run.
  (
    window as unknown as {
      __TAURI_OS_PLUGIN_INTERNALS__?: { platform: 'macos' };
    }
  ).__TAURI_OS_PLUGIN_INTERNALS__ = { platform: 'macos' };

  // Test hook: let an e2e simulate an EXTERNAL edit landing on disk (bumping the
  // file's hash) so a subsequent save trips the guard — the watcher's own event
  // channel isn't available under a pure IPC mock — and read back what the shell
  // actually sent, so an absence can be asserted rather than assumed.
  (window as unknown as { __E2E__?: E2EHooks }).__E2E__ = {
    writeDisk: (path, content) => {
      writeDisk(path, content);
    },
    sent: () => SENT.map((entry) => ({ ...entry })),
    clearSent: () => {
      SENT.length = 0;
    },
  };
  mockWindows('main');

  // Refuse obsolete acceptance URLs instead of silently answering an old
  // native-engine check from this synthetic fixture.
  if (new URLSearchParams(window.location.search).has('realengine')) {
    throw new Error('This obsolete acceptance mode is unavailable. Use reader-owned native service acceptance.');
  }

  mockIPC((cmd, args) => {
    SENT.push({ cmd, args });
    switch (cmd) {
      case 'reader_embedding_policy':
        return 'balanced';
      case 'native_provider_status':
        return {
          availability: ['google_drive', 'dropbox'].map((provider) => ({
            provider, configured: false, supported: true,
            diagnostic: 'This provider is not configured in this build.',
          })),
          sources: [], pendingAdmissions: [], error: null,
        };
      case 'reader_account': {
        const operation = (args as { operation: { operation: string } }).operation;
        if (operation.operation !== 'status') throw { kind: 'unconfigured' };
        return { operation: 'status', status: {
          configured: false, recoveryAvailable: false, retainedLibrary: false,
          connected: false, custody: null, latestJob: null,
        } };
      }
      case 'list_library_sources':
        return mockLibrarySnapshot();
      case 'pick_and_add_library_source': {
        const added = !mockLibraryAdded;
        mockLibraryAdded = true;
        return { source: mockLibrarySource(), added };
      }
      case 'reconcile_library_restore': {
        const sessionRoot = (args as { sessionRoot: string | null }).sessionRoot;
        return mockLibraryAdded && sessionRoot === '/mock/ws' ? mockLibrarySource() : null;
      }
      case 'watch_library_source':
        if ((args as { sourceId: string }).sourceId !== MOCK_LIBRARY_ID) {
          throw { kind: 'source-not-found' };
        }
        return { root: '/mock/ws', generation: 1 };
      case 'grant_library_source':
        if ((args as { sourceId: string }).sourceId !== MOCK_LIBRARY_ID) {
          throw { kind: 'source-not-found' };
        }
        return '/mock/ws';
      case 'mark_library_source_active': {
        if ((args as { sourceId: string }).sourceId !== MOCK_LIBRARY_ID) {
          throw { kind: 'source-not-found' };
        }
        mockLibraryActive = true;
        return mockLibrarySnapshot();
      }
      case 'forget_library_source':
        if (mockLibraryActive) throw { kind: 'active-source' };
        mockLibraryAdded = false;
        return mockLibrarySnapshot();
      case 'diagnostics_info':
        return mockDiagnosticInfo();
      case 'set_diagnostics_level':
        mockDiagnosticLevel = (args as {
          level: 'errors' | 'standard' | 'verbose';
        }).level;
        return mockDiagnosticInfo();
      case 'record_diagnostic_event':
      case 'reveal_diagnostics':
        return null;
      case 'export_diagnostics':
        return {
          status: 'exported',
          path: '/Users/reader/Desktop/mdreader-diagnostics.txt',
          bytes_written: 2048,
        };
      case 'clear_diagnostics':
        if (!(args as { confirmed: boolean }).confirmed) {
          throw { kind: 'confirmation-required' };
        }
        return mockDiagnosticInfo();
      case 'pick_workspace_folder':
        return '/mock/ws';
      case 'set_workspace':
        return (args as { root: string }).root;
      case 'list_workspace_files':
        // The OS refused the walk. Thrown as a plain (non-`Error`) value so it
        // travels the generated `typedError` path exactly as a real backend
        // rejection does, carrying the tagged `ListingError::Unreadable`.
        if (UNREADABLE_FOLDER) {
          throw {
            kind: 'unreadable',
            path: '/mock/ws',
            reason: UNREADABLE_REASON,
            remedy: UNREADABLE_REMEDY,
          };
        }
        // Read fine; there simply is no markdown here.
        if (EMPTY_FOLDER) return { entries: [], from_index: false };
        // Still blocked on the OS — no answer, and therefore nothing to claim.
        if (LISTING_PENDING) return new Promise(() => {});
        // Post-import the shell re-lists from the index; the mock corpus stands
        // in for both, flagged as index-backed so the tree renders identically.
        // With no model there is no index, so the listing is the live fs scan —
        // the same files, read straight off disk, which is precisely why the
        // workspace stays usable. Same while the model is still loading: the
        // index is open (P7.2) but nothing has been imported into it yet.
        return { entries: fileEntries(), from_index: !MODEL_UNAVAILABLE && !MODEL_LOADING };
      case 'start_import':
        // The model is still loading: the command has NOT answered yet, and will
        // not for as long as the load takes. Nothing the shell shows may wait on
        // this promise.
        if (MODEL_LOADING) return new Promise(() => {});
        // Resolve after a beat so the StatusBar's optimistic "Indexing…" (busy)
        // phase is observable before the final status lands — a busy→settled
        // transition without the shell needing the (best-effort) channel feed.
        return new Promise((resolve) =>
          setTimeout(
            () =>
              resolve(
                MODEL_UNAVAILABLE
                  ? modelUnavailableProgress()
                  : {
                      phase: 'done',
                      files_done: 3,
                      files_total: 3,
                      chunks_embedded: 6,
                      error: null,
                    },
              ),
            400,
          ),
        );
      case 'import_status':
        if (MODEL_LOADING) return modelLoadingProgress();
        return MODEL_UNAVAILABLE
          ? modelUnavailableProgress()
          : { phase: 'idle', files_done: 0, files_total: 0, chunks_embedded: 0, error: null };
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
      case 'query_federated_document':
        return federatedDocument((args as { path: string }).path);
      case 'cancel_federated_document':
        return null;
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
        // Mirrors the Rust refusal: the thing being explained IS the fused
        // ranking, so with no semantic arm there is nothing to explain.
        if (MODEL_UNAVAILABLE) {
          throw new Error(
            `the embedding model could not be loaded — ${MODEL_FAILURE_REASON}; a hybrid ranking cannot be explained without its semantic arm`,
          );
        }
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
