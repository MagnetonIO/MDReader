/** The collapsible left rail: source switcher, virtualized file tree, recents.
 * A quiet rail — muted surface, thin borders — that never competes with the
 * reading column. */

import type { ReactNode } from 'react';
import type { ListingFailure, WorkspaceFile } from '../platform/fs.js';
import { FolderIcon } from './icons.js';
import { FileTree } from './FileTree.js';
import { documentKey, type DocumentRef } from '../stores/types.js';
export interface SidebarProps {
  collapsed: boolean;
  root: string | null;
  files: WorkspaceFile[];
  recent: DocumentRef[];
  activePath: string | null;
  loading: boolean;
  /** Why the listing failed, or `null`. Takes precedence over the file tree:
   *  a folder we could not read must never be drawn as a folder with no files. */
  listingError?: ListingFailure | null;
  onPickFolder: () => void;
  /** Native selected sources do not participate in local folder grants. */
  providerSources?: ReactNode;
  onOpenFile: (file: WorkspaceFile) => void;
  onOpenRecent: (ref: DocumentRef) => void;
  isIgnored?: (path: string) => boolean;
}

function basename(path: string): string {
  return path.replace(/[\\/]+$/, '').split(/[\\/]/).pop() ?? path;
}

/**
 * The listing-failure panel that stands in for the tree. It states the fact
 * ("could not be read"), the OS's own reason, and — for a permission denial —
 * the exact grant the user has to make. Deliberately NOT the empty state: the
 * folder may well be full, we simply were not allowed to look.
 */
function ListingErrorPanel({ failure }: { failure: ListingFailure }) {
  if (failure.kind === 'unreadable') {
    return (
      <div className="filetree filetree--error" role="alert" aria-label="Workspace files">
        <p className="filetree__error-title">This folder could not be read.</p>
        <p className="filetree__error-path" title={failure.path}>
          {failure.path}
        </p>
        <p className="filetree__error-reason">{failure.reason}</p>
        <p className="filetree__error-remedy">{failure.remedy}</p>
      </div>
    );
  }
  return (
    <div className="filetree filetree--error" role="alert" aria-label="Workspace files">
      <p className="filetree__error-title">The file list could not be loaded.</p>
      <p className="filetree__error-reason">{failure.message}</p>
    </div>
  );
}

export function Sidebar({
  collapsed,
  root,
  files,
  recent,
  activePath,
  loading,
  listingError,
  onPickFolder,
  providerSources,
  onOpenFile,
  onOpenRecent,
  isIgnored,
}: SidebarProps) {
  return (
    <aside
      className={`sidebar${providerSources ? ' sidebar--reader-library' : ''}${collapsed ? ' sidebar--collapsed' : ''}`}
      aria-hidden={collapsed}
      inert={collapsed ? true : undefined}
    >
      {(!providerSources || root) ? <div className="source-switcher">
        <FolderIcon width="15" height="15" style={{ color: 'var(--color-faint)', flex: 'none' }} />
        <span className="source-switcher__name" title={root ?? undefined}>
          {root ? basename(root) : 'No folder'}
        </span>
        <button type="button" className="icon-btn" onClick={onPickFolder} title="Open folder…">
          {root ? 'Change' : 'Open…'}
        </button>
      </div> : null}

      {root && listingError ? (
        <ListingErrorPanel failure={listingError} />
      ) : root && loading && files.length === 0 ? (
        // The listing has not answered YET. Saying "no markdown files" here is a
        // claim we have not earned — and it is exactly what the app shows while
        // macOS holds the privacy prompt open, since that syscall blocks until
        // the user answers it. An in-flight read reports itself as in-flight.
        <div className="filetree" role="tree" aria-label="Workspace files">
          <p className="filetree__empty">Loading…</p>
        </div>
      ) : root ? (
        <FileTree
          files={files}
          root={root}
          activePath={activePath}
          onOpen={onOpenFile}
          isIgnored={isIgnored}
        />
      ) : !providerSources ? (
        <div className="filetree" role="tree" aria-label="Workspace files">
          <p className="filetree__empty">
            {loading ? 'Loading…' : 'Open a folder to browse its markdown.'}
          </p>
        </div>
      ) : null}

      {providerSources}

      {recent.length > 0 ? (
        <nav className="recent" aria-label="Recent documents">
          <div className="recent__heading">Recent</div>
          {recent.map((ref) => (
            <button
              key={documentKey(ref)}
              type="button"
              className="recent__item"
              onClick={() => onOpenRecent(ref)}
              title={ref.kind === 'cloud' ? `${ref.name} — ${ref.sourceId}` : ref.path}
            >
              {ref.name}
            </button>
          ))}
        </nav>
      ) : null}
    </aside>
  );
}
