import { describe, expect, test } from 'vitest';

import { fuzzyScore, rankFiles, type PaletteFile } from './fuzzy.js';

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
    expect(rankFiles(files, 'zettel')[0]!.file.path).toContain('zettel');
  });

  test('a name hit outranks the same subsequence buried in a path', () => {
    const withKatex = [file('/ws/katex.md'), file('/ws/k/a/t/e/x/other.md')];
    expect(rankFiles(withKatex, 'katex')[0]!.file.name).toBe('katex.md');
  });

  test('drops non-matches', () => {
    expect(rankFiles(files, 'zzzz')).toHaveLength(0);
  });
});
