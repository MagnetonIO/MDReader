import { describe, expect, test } from 'vitest';

import type { WorkspaceFile } from '../platform/fs.js';
import { filterFiles, fuzzyScore, rankFiles } from './file-jump.js';

function file(path: string): WorkspaceFile {
  const name = path.split('/').pop()!;
  return {
    path,
    name,
    dir: path.slice(0, path.length - name.length - 1),
    mtime: 0,
    size: 0,
    ignored: false,
  };
}

const files = [
  file('/ws/docs/guides/mermaid-flowchart.md'),
  file('/ws/docs/guides/katex-block.md'),
  file('/ws/docs/reference/headings-deep.md'),
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

  test('rewards word-boundary matches', () => {
    const boundary = fuzzyScore('gh', 'graph-hub.md')!; // g@0, h@ after '-'
    expect(boundary).toBeGreaterThan(0);
  });

  test('empty query scores zero', () => {
    expect(fuzzyScore('', 'anything.md')).toBe(0);
  });
});

describe('rankFiles', () => {
  test('an empty query returns all files (capped)', () => {
    expect(rankFiles(files, '').map((m) => m.file.name)).toHaveLength(files.length);
  });

  test('ranks the best name match first', () => {
    const ranked = filterFiles(files, 'mermaid');
    expect(ranked[0]!.name).toBe('mermaid-flowchart.md');
  });

  test('matches across the path, not just the name', () => {
    const ranked = filterFiles(files, 'zettel');
    expect(ranked[0]!.path).toContain('zettel');
  });

  test('a name hit outranks the same subsequence buried in a path', () => {
    const withKatex = [
      file('/ws/katex.md'),
      file('/ws/k/a/t/e/x/other.md'),
    ];
    const ranked = filterFiles(withKatex, 'katex');
    expect(ranked[0]!.name).toBe('katex.md');
  });

  test('drops non-matches', () => {
    expect(filterFiles(files, 'zzzz')).toHaveLength(0);
  });

  test('honours the limit', () => {
    expect(rankFiles(files, '', 2)).toHaveLength(2);
  });
});
