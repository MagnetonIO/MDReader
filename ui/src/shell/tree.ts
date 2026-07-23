/**
 * Turn the flat `WorkspaceFile[]` from the backend into a directory tree, then
 * flatten it (respecting an expanded set) into the row list the virtualized
 * FileTree renders. Pure and testable — no React, no DOM.
 */

import type { WorkspaceFile } from '../platform/fs.js';

export interface TreeDir {
  type: 'dir';
  name: string;
  path: string;
  children: TreeNode[];
}

export interface TreeFile {
  type: 'file';
  name: string;
  path: string;
  file: WorkspaceFile;
}

export type TreeNode = TreeDir | TreeFile;

function splitSegments(file: WorkspaceFile, root: string): string[] {
  const normalizedRoot = root.replace(/[\\/]+$/, '');
  let rel = file.path;
  if (rel.startsWith(normalizedRoot)) rel = rel.slice(normalizedRoot.length);
  return rel.split(/[\\/]+/).filter((segment) => segment.length > 0);
}

/** Build a sorted tree (directories first, then files, each alphabetical). */
export function buildTree(files: WorkspaceFile[], root: string): TreeNode[] {
  const rootDir: TreeDir = { type: 'dir', name: '', path: root, children: [] };
  const dirIndex = new Map<string, TreeDir>([[root, rootDir]]);

  for (const file of files) {
    const segments = splitSegments(file, root);
    if (segments.length === 0) continue;

    let parent = rootDir;
    let cumulative = root.replace(/[\\/]+$/, '');
    for (let i = 0; i < segments.length - 1; i += 1) {
      const segment = segments[i]!;
      cumulative = `${cumulative}/${segment}`;
      let dir = dirIndex.get(cumulative);
      if (!dir) {
        dir = { type: 'dir', name: segment, path: cumulative, children: [] };
        dirIndex.set(cumulative, dir);
        parent.children.push(dir);
      }
      parent = dir;
    }

    const name = segments[segments.length - 1]!;
    parent.children.push({ type: 'file', name, path: file.path, file });
  }

  sortDir(rootDir);
  return rootDir.children;
}

function sortDir(dir: TreeDir): void {
  dir.children.sort((a, b) => {
    if (a.type !== b.type) return a.type === 'dir' ? -1 : 1;
    return a.name.localeCompare(b.name);
  });
  for (const child of dir.children) {
    if (child.type === 'dir') sortDir(child);
  }
}

export interface FlatRow {
  node: TreeNode;
  depth: number;
  /** For a directory row, whether it is currently expanded. */
  expanded: boolean;
}

/** Depth-first flatten, descending into expanded directories only. */
export function flattenTree(
  nodes: TreeNode[],
  expanded: Set<string>,
  depth = 0,
): FlatRow[] {
  const rows: FlatRow[] = [];
  for (const node of nodes) {
    if (node.type === 'dir') {
      const isExpanded = expanded.has(node.path);
      rows.push({ node, depth, expanded: isExpanded });
      if (isExpanded) rows.push(...flattenTree(node.children, expanded, depth + 1));
    } else {
      rows.push({ node, depth, expanded: false });
    }
  }
  return rows;
}

/** All directory paths in the tree (e.g. to expand everything by default). */
export function collectDirPaths(nodes: TreeNode[]): string[] {
  const paths: string[] = [];
  for (const node of nodes) {
    if (node.type === 'dir') {
      paths.push(node.path);
      paths.push(...collectDirPaths(node.children));
    }
  }
  return paths;
}
