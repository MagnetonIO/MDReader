/** The collapsible left rail: source switcher, virtualized file tree, recents.
 * A quiet rail — muted surface, thin borders — that never competes with the
 * reading column. */

import type { WorkspaceFile } from '../platform/fs.js';
import { FolderIcon } from './icons.js';
import { FileTree } from './FileTree.js';
import type { DocRef } from '../stores/types.js';

export interface SidebarProps {
  collapsed: boolean;
  root: string | null;
  files: WorkspaceFile[];
  recent: DocRef[];
  activePath: string | null;
  loading: boolean;
  onPickFolder: () => void;
  onOpenFile: (file: WorkspaceFile) => void;
  onOpenRecent: (ref: DocRef) => void;
  isIgnored?: (path: string) => boolean;
}

function basename(path: string): string {
  return path.replace(/[\\/]+$/, '').split(/[\\/]/).pop() ?? path;
}

export function Sidebar({
  collapsed,
  root,
  files,
  recent,
  activePath,
  loading,
  onPickFolder,
  onOpenFile,
  onOpenRecent,
  isIgnored,
}: SidebarProps) {
  return (
    <aside
      className={`sidebar${collapsed ? ' sidebar--collapsed' : ''}`}
      aria-hidden={collapsed}
      inert={collapsed ? true : undefined}
    >
      <div className="source-switcher">
        <FolderIcon width="15" height="15" style={{ color: 'var(--color-faint)', flex: 'none' }} />
        <span className="source-switcher__name" title={root ?? undefined}>
          {root ? basename(root) : 'No folder'}
        </span>
        <button type="button" className="icon-btn" onClick={onPickFolder} title="Open folder…">
          {root ? 'Change' : 'Open…'}
        </button>
      </div>

      {root ? (
        <FileTree
          files={files}
          root={root}
          activePath={activePath}
          onOpen={onOpenFile}
          isIgnored={isIgnored}
        />
      ) : (
        <div className="filetree" role="tree" aria-label="Workspace files">
          <p className="filetree__empty">
            {loading ? 'Loading…' : 'Open a folder to browse its markdown.'}
          </p>
        </div>
      )}

      {recent.length > 0 ? (
        <nav className="recent" aria-label="Recent documents">
          <div className="recent__heading">Recent</div>
          {recent.map((ref) => (
            <button
              key={ref.path}
              type="button"
              className="recent__item"
              onClick={() => onOpenRecent(ref)}
              title={ref.path}
            >
              {ref.name}
            </button>
          ))}
        </nav>
      ) : null}
    </aside>
  );
}
