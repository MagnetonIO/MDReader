/** The Cmd+P file-jump palette. cmdk gives accessible list semantics + arrow/Enter
 * navigation; we supply our own fuzzy ranking (cmdk's own filter is disabled). */

import { useMemo, useState } from 'react';
import { Command } from 'cmdk';

import type { WorkspaceFile } from '../platform/fs.js';
import { rankFiles } from './file-jump.js';

export interface FileJumpPaletteProps {
  files: WorkspaceFile[];
  root: string | null;
  onOpen: (file: WorkspaceFile) => void;
  onClose: () => void;
}

function relativePath(path: string, root: string | null): string {
  if (!root) return path;
  const normalized = root.replace(/[\\/]+$/, '');
  return path.startsWith(normalized) ? path.slice(normalized.length + 1) : path;
}

export function FileJumpPalette({ files, root, onOpen, onClose }: FileJumpPaletteProps) {
  const [query, setQuery] = useState('');
  const ranked = useMemo(() => rankFiles(files, query, 100), [files, query]);

  return (
    <div
      className="overlay-backdrop"
      role="presentation"
      onMouseDown={onClose}
      onKeyDown={(event) => {
        if (event.key === 'Escape') onClose();
      }}
    >
      <div className="palette" onMouseDown={(event) => event.stopPropagation()}>
        <Command shouldFilter={false} loop label="Jump to file">
          <Command.Input
            className="palette__input"
            value={query}
            onValueChange={setQuery}
            placeholder="Jump to a file…"
            autoFocus
          />
          <Command.List className="palette__list">
            <Command.Empty className="palette__empty">No matching files</Command.Empty>
            {ranked.map(({ file }) => (
              <Command.Item
                key={file.path}
                value={file.path}
                className="palette__item"
                onSelect={() => onOpen(file)}
              >
                <span className="palette__name">{file.name}</span>
                <span className="palette__path">{relativePath(file.path, root)}</span>
              </Command.Item>
            ))}
          </Command.List>
        </Command>
      </div>
    </div>
  );
}
