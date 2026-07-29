/** The collapsible left rail: source switcher, virtualized file tree, recents.
 * A quiet rail — muted surface, thin borders — that never competes with the
 * reading column. */

import type { ListingFailure, WorkspaceFile } from '../platform/fs.js';
import type {
  EntityRow,
  LibraryEntityQueryResponse_Serialize,
  LibrarySourceExecution,
} from '../bindings.js';
import { DropboxIcon, FolderIcon, GoogleDriveIcon } from './icons.js';
import { FileTree } from './FileTree.js';
import type { DocRef } from '../stores/types.js';
import {
  libraryDocumentTitle,
  rowsForMember,
  type LibraryAvailability,
} from './library-documents.js';

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
  libraryEvidence?: LibraryEntityQueryResponse_Serialize[];
  librarySearch?: string;
  libraryLoading?: boolean;
  libraryError?: string | null;
  libraryAvailability?: LibraryAvailability;
  libraryHasMore?: boolean;
  onLibrarySearch?: (value: string) => void;
  onOpenLibrary?: (member: LibrarySourceExecution, row: EntityRow) => void;
  onRefreshLibrary?: () => void;
  onLoadMoreLibrary?: () => void;
}

function basename(path: string): string {
  return path.replace(/[\\/]+$/, '').split(/[\\/]/).pop() ?? path;
}

function sourceIcon(kind: LibrarySourceExecution['kind']) {
  switch (kind) {
    case 'google-drive':
      return <GoogleDriveIcon className="library-sources__provider-icon" />;
    case 'dropbox':
      return <DropboxIcon className="library-sources__provider-icon" />;
    default:
      return null;
  }
}

function LibraryAvailabilityBadge({ availability }: { availability: LibraryAvailability }) {
  if (availability.kind === 'ready') return null;
  if (availability.kind === 'failed') {
    return <p className="library-sources__error" role="alert">{availability.message}</p>;
  }
  const names = availability.sourceNames.join(', ');
  const sourceLabel = names || 'Library sources';
  const message =
    availability.kind === 'syncing'
      ? `Syncing published metadata for ${sourceLabel}.`
      : availability.kind === 'stale'
        ? `Showing the last published snapshot; ${sourceLabel} needs refresh.`
        : `Showing the last published snapshot; ${sourceLabel} needs attention.`;
  return <p className="library-sources__state" role="status">{message}</p>;
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
  libraryEvidence = [],
  librarySearch = '',
  libraryLoading = false,
  libraryError = null,
  libraryAvailability = { kind: 'ready' },
  libraryHasMore = false,
  onLibrarySearch,
  onOpenLibrary,
  onRefreshLibrary,
  onLoadMoreLibrary,
}: SidebarProps) {
  const search = librarySearch.trim().toLocaleLowerCase();
  const hasSupplementalMemberships = libraryEvidence.some((evidence) =>
    evidence.memberships.some(
      (member) => member.kind !== 'local-folder' || !member.active,
    ),
  );
  const hasFilterableActiveWorkspace = Boolean(root && onLibrarySearch);
  const showLibraryRail =
    hasFilterableActiveWorkspace ||
    hasSupplementalMemberships ||
    libraryLoading ||
    libraryHasMore ||
    Boolean(libraryError) ||
    libraryAvailability.kind !== 'ready';
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
          filter={librarySearch}
          isIgnored={isIgnored}
        />
      ) : (
        <div className="filetree" role="tree" aria-label="Workspace files">
          <p className="filetree__empty">
            {loading ? 'Loading…' : 'Open a folder to browse its markdown.'}
          </p>
        </div>
      )}

      {showLibraryRail ? (
        <nav className="library-sources" aria-label="Library sources">
          <div className="library-sources__heading">
            <span>Library</span>
            {onRefreshLibrary ? (
              <button
                type="button"
                className="library-sources__refresh"
                onClick={onRefreshLibrary}
                disabled={libraryLoading}
              >
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
          <LibraryAvailabilityBadge availability={libraryAvailability} />
          {libraryEvidence.flatMap((evidence) =>
            evidence.memberships
              .filter((member) => member.kind !== 'local-folder' || !member.active)
              .map((member) => {
                const rows = rowsForMember(evidence, member).filter((row) => {
                  if (!search) return true;
                  return libraryDocumentTitle(row).toLocaleLowerCase().includes(search);
                });
                return (
                  <section className="library-sources__source" key={member.membership_id}>
                    <div>
                      {sourceIcon(member.kind)}
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
                          const title = libraryDocumentTitle(row);
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
              }),
          )}
          {libraryEvidence.length === 0 && libraryLoading ? <p>Reading Library…</p> : null}
          {libraryHasMore && onLoadMoreLibrary ? (
            <button
              type="button"
              className="library-sources__refresh"
              onClick={onLoadMoreLibrary}
              disabled={libraryLoading}
            >
              {libraryLoading ? 'Reading…' : 'Load more'}
            </button>
          ) : null}
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
