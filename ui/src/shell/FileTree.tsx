/**
 * Virtualized workspace file tree. The backend hands us a flat file list; we
 * build a directory tree (see {@link ./tree}) and flatten it against an expanded
 * set for windowed rendering via `@tanstack/react-virtual`. Rows are buttons
 * (Tab-focusable); the container adds arrow-key navigation for a keyboard-first
 * feel. `.md`/`.markdown` files only; gitignored entries are dimmed.
 */

import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import type { KeyboardEvent as ReactKeyboardEvent } from 'react';
import { useVirtualizer } from '@tanstack/react-virtual';

import type { WorkspaceFile } from '../platform/fs.js';
import { ChevronRight, DocIcon, FolderIcon } from './icons.js';
import { buildTree, collectDirPaths, flattenTree } from './tree.js';
import type { FlatRow } from './tree.js';

const ROW_HEIGHT = 26;

export interface FileTreeProps {
  files: WorkspaceFile[];
  root: string;
  activePath: string | null;
  onOpen: (file: WorkspaceFile) => void;
  /** The Library document filter shared with supplemental source rows. */
  filter?: string;
  /** Predicate marking a path as gitignored (dimmed). */
  isIgnored?: (path: string) => boolean;
}

export function FileTree({
  files,
  root,
  activePath,
  onOpen,
  filter = '',
  isIgnored,
}: FileTreeProps) {
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const search = filter.trim().toLocaleLowerCase();
  const isFiltering = search.length > 0;
  const unfilteredTree = useMemo(() => buildTree(files, root), [files, root]);
  const visibleFiles = useMemo(() => {
    if (!isFiltering) return files;
    return files.filter((file) => file.name.toLocaleLowerCase().includes(search));
  }, [files, isFiltering, search]);
  const tree = useMemo(
    () => (isFiltering ? buildTree(visibleFiles, root) : unfilteredTree),
    [isFiltering, root, unfilteredTree, visibleFiles],
  );

  // Default: expand every directory so the corpus is visible at a glance.
  const [expanded, setExpanded] = useState<Set<string>>(
    () => new Set(collectDirPaths(unfilteredTree)),
  );
  const effectiveExpanded = useMemo(() => {
    if (!isFiltering) return expanded;
    const forced = new Set(expanded);
    for (const path of collectDirPaths(tree)) forced.add(path);
    return forced;
  }, [expanded, isFiltering, tree]);

  const rows = useMemo(() => flattenTree(tree, effectiveExpanded), [tree, effectiveExpanded]);

  const [focusIndex, setFocusIndex] = useState(0);
  const pendingFocus = useRef<number | null>(null);
  useEffect(() => {
    setFocusIndex((current) => Math.min(current, Math.max(rows.length - 1, 0)));
  }, [rows.length]);

  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_HEIGHT,
    overscan: 12,
  });

  function toggle(path: string) {
    if (isFiltering) return;
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  }

  function activateRow(row: FlatRow) {
    if (row.node.type === 'dir') toggle(row.node.path);
    else onOpen(row.node.file);
  }

  function moveFocus(index: number) {
    const clamped = Math.min(Math.max(index, 0), rows.length - 1);
    setFocusIndex(clamped);
    pendingFocus.current = clamped;
    virtualizer.scrollToIndex(clamped, { align: 'auto' });
  }

  // After a virtualized row for the pending focus mounts, move DOM focus to it.
  useLayoutEffect(() => {
    if (pendingFocus.current === null) return;
    const target = scrollRef.current?.querySelector<HTMLElement>(
      `[data-tree-index="${pendingFocus.current}"]`,
    );
    if (target) {
      target.focus();
      pendingFocus.current = null;
    }
  });

  function onKeyDown(event: ReactKeyboardEvent) {
    const row = rows[focusIndex];
    switch (event.key) {
      case 'ArrowDown':
        event.preventDefault();
        moveFocus(focusIndex + 1);
        break;
      case 'ArrowUp':
        event.preventDefault();
        moveFocus(focusIndex - 1);
        break;
      case 'ArrowRight':
        event.preventDefault();
        if (row?.node.type === 'dir' && !row.expanded) toggle(row.node.path);
        else moveFocus(focusIndex + 1);
        break;
      case 'ArrowLeft':
        event.preventDefault();
        if (row?.node.type === 'dir' && row.expanded) toggle(row.node.path);
        else moveFocus(focusIndex - 1);
        break;
      case 'Home':
        event.preventDefault();
        moveFocus(0);
        break;
      case 'End':
        event.preventDefault();
        moveFocus(rows.length - 1);
        break;
      case 'Enter':
      case ' ':
        event.preventDefault();
        if (row) activateRow(row);
        break;
      default:
        break;
    }
  }

  if (files.length === 0) {
    return (
      <div className="filetree" role="tree" aria-label="Workspace files">
        <p className="filetree__empty">No markdown files in this folder yet.</p>
      </div>
    );
  }

  if (visibleFiles.length === 0) {
    return (
      <div className="filetree" role="tree" aria-label="Workspace files">
        <p className="filetree__empty">No Library documents match.</p>
      </div>
    );
  }

  return (
    <div
      ref={scrollRef}
      className="filetree"
      role="tree"
      aria-label="Workspace files"
      onKeyDown={onKeyDown}
    >
      <div style={{ height: virtualizer.getTotalSize(), position: 'relative' }}>
        {virtualizer.getVirtualItems().map((item) => {
          const row = rows[item.index]!;
          const { node } = row;
          const isDir = node.type === 'dir';
          const isActive = !isDir && node.path === activePath;
          const ignored = isIgnored?.(node.path) ?? false;
          return (
            <button
              key={node.path}
              type="button"
              role="treeitem"
              data-tree-index={item.index}
              aria-expanded={isDir ? row.expanded : undefined}
              aria-selected={isActive}
              tabIndex={item.index === focusIndex ? 0 : -1}
              className={[
                'filetree__row',
                isDir ? 'filetree__row--dir' : '',
                isActive ? 'filetree__row--active' : '',
                ignored ? 'filetree__row--ignored' : '',
              ]
                .filter(Boolean)
                .join(' ')}
              style={{
                position: 'absolute',
                top: 0,
                left: 0,
                width: '100%',
                height: ROW_HEIGHT,
                transform: `translateY(${item.start}px)`,
                paddingLeft: `calc(var(--space-2) + ${row.depth * 0.9}rem)`,
              }}
              onClick={() => {
                setFocusIndex(item.index);
                activateRow(row);
              }}
              title={node.path}
            >
              {isDir ? (
                <ChevronRight
                  className={`filetree__twisty${row.expanded ? ' filetree__twisty--open' : ''}`}
                  width="12"
                  height="12"
                />
              ) : (
                <span className="filetree__twisty" />
              )}
              <span className="filetree__icon">
                {isDir ? <FolderIcon width="14" height="14" /> : <DocIcon width="14" height="14" />}
              </span>
              <span className="filetree__label">{node.name}</span>
            </button>
          );
        })}
      </div>
    </div>
  );
}
