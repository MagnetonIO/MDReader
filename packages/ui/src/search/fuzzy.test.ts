import { describe, expect, test } from 'vitest';

import { fuzzyScore, rankFiles, type PaletteFile } from './fuzzy.js';
import { fileContext } from './file-context.js';

function file(path: string): PaletteFile {
  return { path, name: path.split('/').pop()! };
}

const files = [
  file('/ws/docs/guides/mermaid-flowchart.md'),
  file('/ws/docs/guides/katex-block.md'),
  file('/ws/notes/zettel/graph-hub.md'),
  file('/ws/index.md'),
];

describe('fuzzyScore', () => {
  test('returns null when not a subsequence', () => {
    expect(fuzzyScore('xyz', 'index.md')).toBeNull();
  });

  test('scores a contiguous prefix higher than a scattered match', () => {
    const contiguous = fuzzyScore('kat', 'katex-block.md')!;
    const scattered = fuzzyScore('kat', 'breakfast-atlas.md')!;
    expect(contiguous).toBeGreaterThan(scattered);
  });

  test('rewards word-boundary matches and empty scores zero', () => {
    expect(fuzzyScore('gh', 'graph-hub.md')!).toBeGreaterThan(0);
    expect(fuzzyScore('', 'anything.md')).toBe(0);
  });
});

describe('rankFiles', () => {
  test('empty query returns all files (capped)', () => {
    expect(rankFiles(files, '')).toHaveLength(files.length);
    expect(rankFiles(files, '', 2)).toHaveLength(2);
  });

  test('ranks the best name match first and matches across the path', () => {
    expect(rankFiles(files, 'mermaid')[0]!.file.name).toBe('mermaid-flowchart.md');
    expect(rankFiles(files, 'zettel', 100, '/ws')[0]!.file.path).toContain('zettel');
  });

  test('a name hit outranks the same subsequence buried in a path', () => {
    const withKatex = [file('/ws/katex.md'), file('/ws/k/a/t/e/x/other.md')];
    expect(rankFiles(withKatex, 'katex', 100, '/ws')[0]!.file.name).toBe('katex.md');
  });

  test('drops non-matches', () => {
    expect(rankFiles(files, 'zzzz')).toHaveLength(0);
  });

  test('keeps word/folder abbreviations without incidental scattered filename letters', () => {
    expect(rankFiles(files, 'gh')[0]!.file.name).toBe('graph-hub.md');
    expect(rankFiles([file('/ws/docs/guides/README.md')], 'dgr', 100, '/ws')).toHaveLength(1);
    expect(rankFiles([file('/ws/README.md')], 'rad', 100, '/ws')).toEqual([]);
  });

  test('workspace and ancestor names do not match unrelated READMEs', () => {
    const candidates = [file('/private/rad-belden/docs/README.md'), file('/private/rad-belden/sql/README.md'), file('/private/rad-belden/docs/rad.md')];
    expect(rankFiles(candidates, 'rad', 100, '/private/rad-belden').map(({ file }) => file.path)).toEqual([candidates[2]!.path]);
    expect(rankFiles(candidates, 'private', 100, '/private/rad-belden')).toEqual([]);
    expect(rankFiles(candidates, 'docs/read', 100, '/private/rad-belden')[0]!.file).toBe(candidates[0]);
  });

  test('relative context distinguishes duplicates without changing exact identity', () => {
    expect(fileContext('/ws/docs/design/README.md', '/ws/')).toEqual({ relativePath: 'docs/design/README.md', parentLabel: 'docs/design' });
    expect(fileContext('/ws/a/b/c/README.md', '/ws').parentLabel).toBe('…/b/c');
    expect(fileContext('/ws/README.md', '/ws').parentLabel).toBe('Workspace root');
    const exact = file('/ws/docs/README.md');
    expect(rankFiles([exact], 'read', 100, '/ws')[0]!.file).toBe(exact);
  });

  test('outside-root, prefix siblings, traversal and unknown scope match filenames only', () => {
    for (const path of ['/ws-other/notes/README.md', '/elsewhere/notes/README.md', '/ws/../notes/README.md']) {
      const candidate = file(path);
      expect(rankFiles([candidate], 'notes', 100, '/ws')).toEqual([]);
      expect(rankFiles([candidate], 'read', 100, '/ws')[0]!.file.path).toBe(path);
      expect(fileContext(path, '/ws').relativePath).toBeNull();
    }
    expect(rankFiles([file('/ws/notes/README.md')], 'notes')).toEqual([]);
    expect(fileContext('/WS/notes/README.md', '/ws').relativePath).toBeNull();
  });

  test('Windows drive/UNC separators are display-only and root matching is segment bounded', () => {
    const win = { path: 'C:\\Projects\\Rad\\docs\\README.md', name: 'README.md' };
    expect(fileContext(win.path, 'c:\\projects\\rad\\').relativePath).toBe('docs/README.md');
    expect(rankFiles([win], 'docs\\read', 100, 'c:/projects/rad')[0]!.file.path).toBe(win.path);
    expect(rankFiles([win], 'rad', 100, 'c:/projects/rad')).toEqual([]);
    expect(fileContext('\\\\server\\share\\notes\\README.md', '\\\\SERVER\\share').relativePath).toBe('notes/README.md');
    expect(fileContext('C:\\Projects\\Rad-other\\README.md', 'C:\\Projects\\Rad').relativePath).toBeNull();
    expect(fileContext('/docs/README.md', '/').relativePath).toBe('docs/README.md');
  });
});
