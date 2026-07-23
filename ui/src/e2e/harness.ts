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
    '# Welcome\n\nA calm place to read.\n\nSee the [[Guide]] and a [[Nonexistent]] note.\n\n## Getting started\n\nOpen a file from the tree, or press Cmd+P.\n',
  '/mock/ws/docs/guide.md':
    '# Guide\n\nAn introduction.\n\n## Section A\n\nAlpha content.\n\n## Section B\n\nBeta content.\n',
  '/mock/ws/docs/reference.md': '# Reference\n\n## API\n\nReference material.\n',
};

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

/** The engine's wikilink resolution, mocked from a fixed link table. */
function resolveWikilink(target: string, heading: string | null) {
  if (/^guide$/i.test(target.trim())) {
    return {
      status: 'resolved',
      path: '/mock/ws/docs/guide.md',
      name: 'guide.md',
      // `## Section B` sits on line 7 of guide.md.
      line: heading ? 7 : null,
      candidates: [],
    };
  }
  return { status: 'unresolved', path: null, name: null, line: null, candidates: [] };
}

export function installTauriMocks(): void {
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
        // The index "finished" instantly: three files, some chunks embedded.
        return {
          phase: 'done',
          files_done: 3,
          files_total: 3,
          chunks_embedded: 6,
          error: null,
        };
      case 'import_status':
        return { phase: 'idle', files_done: 0, files_total: 0, chunks_embedded: 0, error: null };
      case 'resolve_wikilink': {
        const { target, heading } = args as { target: string; heading: string | null };
        return resolveWikilink(target, heading);
      }
      case 'read_doc': {
        const path = (args as { path: string }).path;
        return { content: CORPUS[path] ?? '# Missing\n', mtime: 1_700_000_000_000, hash: `mock-${path}` };
      }
      case 'watch_workspace':
        return null;
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
