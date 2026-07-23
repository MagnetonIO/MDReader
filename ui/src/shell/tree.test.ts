import { describe, expect, test } from 'vitest';

import type { WorkspaceFile } from '../platform/fs.js';
import { buildTree, collectDirPaths, flattenTree } from './tree.js';
import type { TreeDir } from './tree.js';

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

const root = '/ws';
const files = [
  file('/ws/index.md'),
  file('/ws/docs/guides/katex.md'),
  file('/ws/docs/guides/mermaid.md'),
  file('/ws/docs/reference/api.md'),
];

describe('buildTree', () => {
  test('nests files under their directories', () => {
    const tree = buildTree(files, root);
    // Directories sort before files: [docs, index.md].
    expect(tree.map((n) => n.name)).toEqual(['docs', 'index.md']);
    const docs = tree[0] as TreeDir;
    expect(docs.type).toBe('dir');
    expect(docs.children.map((n) => n.name)).toEqual(['guides', 'reference']);
    const guides = docs.children[0] as TreeDir;
    expect(guides.children.map((n) => n.name)).toEqual(['katex.md', 'mermaid.md']);
  });

  test('directory paths are absolute and cumulative', () => {
    const tree = buildTree(files, root);
    const docs = tree[0] as TreeDir;
    expect(docs.path).toBe('/ws/docs');
    expect((docs.children[0] as TreeDir).path).toBe('/ws/docs/guides');
  });

  test('tolerates a trailing slash on the root', () => {
    const tree = buildTree(files, '/ws/');
    expect(tree.map((n) => n.name)).toEqual(['docs', 'index.md']);
  });
});

describe('flattenTree', () => {
  test('hides children of collapsed directories', () => {
    const tree = buildTree(files, root);
    const rows = flattenTree(tree, new Set());
    // Only top-level rows visible: docs (collapsed) + index.md.
    expect(rows.map((r) => r.node.name)).toEqual(['docs', 'index.md']);
    expect(rows[0]!.depth).toBe(0);
  });

  test('reveals children of expanded directories with increasing depth', () => {
    const tree = buildTree(files, root);
    const rows = flattenTree(tree, new Set(['/ws/docs']));
    const names = rows.map((r) => r.node.name);
    expect(names).toEqual(['docs', 'guides', 'reference', 'index.md']);
    const guidesRow = rows.find((r) => r.node.name === 'guides')!;
    expect(guidesRow.depth).toBe(1);
  });

  test('fully expanded tree lists every file', () => {
    const tree = buildTree(files, root);
    const rows = flattenTree(tree, new Set(collectDirPaths(tree)));
    const fileRows = rows.filter((r) => r.node.type === 'file');
    expect(fileRows.map((r) => r.node.name).sort()).toEqual([
      'api.md',
      'index.md',
      'katex.md',
      'mermaid.md',
    ]);
  });
});
