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
};

// A monotonic per-path "disk version" so the mock can produce a NEW hash whenever
// a file's bytes change — enough to exercise the save hash-guard end to end. The
// real backend hashes bytes; the mock's opaque token is equivalent for the UI,
// which only ever compares hashes for equality.
const DISK_VERSION: Record<string, number> = {};

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

export function installTauriMocks(): void {
  // Test hook: let an e2e simulate an EXTERNAL edit landing on disk (bumping the
  // file's hash) so a subsequent save trips the guard — the watcher's own event
  // channel isn't available under a pure IPC mock.
  (window as unknown as { __E2E__?: { writeDisk: (path: string, content: string) => void } }).__E2E__ =
    { writeDisk };
  mockWindows('main');
  mockIPC((cmd, args) => {
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
