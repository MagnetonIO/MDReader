/** The collapsible left rail: source switcher, virtualized file tree, recents.
 * A quiet rail — muted surface, thin borders — that never competes with the
 * reading column. */

import type { ListingFailure, WorkspaceFile } from '../platform/fs.js';
import type {
  EntityRow,
  LibraryEntityQueryResponse_Serialize,
  LibrarySourceExecution,
} from '../bindings.js';
import { FolderIcon } from './icons.js';
import { FileTree } from './FileTree.js';
import type { DocRef } from '../stores/types.js';
import { rowsForMember } from './library-documents.js';

export interface SidebarProps {
  collapsed: boolean;
  root: string | null;
  files: WorkspaceFile[];
  recent: DocRef[];
  activePath: string | null;
  loading: boolean;
  /** Why the listing failed, or `null`. Takes precedence over the file tree:
   *  a folder we could not read must never be drawn as a folder with no files. */
  listingError?: ListingFailure | null;
  onPickFolder: () => void;
  onOpenFile: (file: WorkspaceFile) => void;
  onOpenRecent: (ref: DocRef) => void;
  isIgnored?: (path: string) => boolean;
  libraryEvidence?: LibraryEntityQueryResponse_Serialize | null;
  librarySearch?: string;
  libraryLoading?: boolean;
  libraryError?: string | null;
  onLibrarySearch?: (value: string) => void;
  onOpenLibrary?: (member: LibrarySourceExecution, row: EntityRow) => void;
  onRefreshLibrary?: () => void;
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
  onOpenFile,
  onOpenRecent,
  isIgnored,
  libraryEvidence = null,
  librarySearch = '',
  libraryLoading = false,
  libraryError = null,
  onLibrarySearch,
  onOpenLibrary,
  onRefreshLibrary,
}: SidebarProps) {
  const search = librarySearch.trim().toLocaleLowerCase();
  const supplementaryMemberships =
    libraryEvidence?.memberships.filter(
      (member) => member.kind !== 'local-folder' || !member.active,
    ) ?? [];
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
      ) : (
        <div className="filetree" role="tree" aria-label="Workspace files">
          <p className="filetree__empty">
            {loading ? 'Loading…' : 'Open a folder to browse its markdown.'}
          </p>
        </div>
      )}

      {supplementaryMemberships.length > 0 ? (
        <nav className="library-sources" aria-label="Library sources">
          <div className="library-sources__heading">
            <span>Library</span>
            {onRefreshLibrary ? (
              <button type="button" onClick={onRefreshLibrary} disabled={libraryLoading}>
                {libraryLoading ? 'Reading…' : 'Refresh'}
              </button>
            ) : null}
          </div>
          {onLibrarySearch ? (
            <input
              type="search"
              value={librarySearch}
              onChange={(event) => onLibrarySearch(event.currentTarget.value)}
              placeholder="Filter Library documents"
              aria-label="Filter Library documents"
            />
          ) : null}
          {libraryError ? <p className="library-sources__error">{libraryError}</p> : null}
          {supplementaryMemberships.map((member) => {
            const rows =
              libraryEvidence
                ? rowsForMember(libraryEvidence, member).filter((row) => {
                    if (!search) return true;
                    const fields = row.fields;
                    const title = typeof fields.title === 'string' ? fields.title : row.record_key;
                    return title.toLocaleLowerCase().includes(search);
                  })
                : [];
            return (
              <section className="library-sources__source" key={member.membership_id}>
                <div>
                  <strong>{member.name}</strong>
                  <small>
                    {member.editable
                      ? member.active
                        ? 'Local · editable'
                        : 'Local · open to edit'
                      : 'Read-only'}
                  </small>
                </div>
                {rows.length > 0 ? (
                  <ul>
                    {rows.map((row) => {
                      const fields = row.fields;
                      const title =
                        typeof fields.title === 'string' ? fields.title : row.record_key;
                      return (
                        <li key={row.record_key}>
                          <button
                            type="button"
                            className="library-sources__document"
                            aria-label={`Open ${title} from ${member.name}`}
                            onClick={() => onOpenLibrary?.(member, row)}
                          >
                            {title}
                          </button>
                        </li>
                      );
                    })}
                  </ul>
                ) : (
                  <p>{search ? 'No Library documents match.' : 'No documents found.'}</p>
                )}
              </section>
            );
          })}
          {!libraryEvidence && libraryLoading ? <p>Reading Library…</p> : null}
        </nav>
      ) : null}

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
