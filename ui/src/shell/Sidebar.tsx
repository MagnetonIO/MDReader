/** The collapsible left rail: source switcher, virtualized file tree, recents.
 * A quiet rail — muted surface, thin borders — that never competes with the
 * reading column. */

import { useState, type PointerEvent as ReactPointerEvent } from 'react';
import type { ListingFailure, WorkspaceFile } from '../platform/fs.js';
import type {
  EntityRow,
  LibraryEntityQueryResponse_Serialize,
  LibrarySourceExecution,
} from '../bindings.js';
import { DropboxIcon, FolderIcon, GoogleDriveIcon } from './icons.js';
import { FileTree } from './FileTree.js';
import { documentKey, type DocumentRef } from '../stores/types.js';
import {
  libraryDocumentTitle,
  rowsForMember,
  type LibraryAvailability,
} from './library-documents.js';

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
  onOpenFile: (file: WorkspaceFile) => void;
  onOpenRecent: (ref: DocumentRef) => void;
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
  onLoadMoreLibrary?: (membershipId?: string) => void;
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
  const [libraryExpanded, setLibraryExpanded] = useState(false);
  const [libraryCollapsed, setLibraryCollapsed] = useState(false);
  const [collapsedLibrarySources, setCollapsedLibrarySources] = useState<Set<string>>(
    () => new Set(),
  );
  const [libraryHeight, setLibraryHeight] = useState<number | null>(null);
  const beginLibraryResize = (event: ReactPointerEvent<HTMLDivElement>) => {
    const rail = event.currentTarget.nextElementSibling as HTMLElement | null;
    const sidebar = event.currentTarget.closest('.sidebar') as HTMLElement | null;
    if (!rail || !sidebar) return;
    const startY = event.clientY;
    const startHeight = rail.getBoundingClientRect().height;
    const minimum = 54;
    const maximum = Math.max(minimum, sidebar.getBoundingClientRect().height * 0.6);
    const pointerId = event.pointerId;
    event.currentTarget.setPointerCapture(pointerId);
    const move = (moveEvent: PointerEvent) => {
      setLibraryHeight(
        Math.min(maximum, Math.max(minimum, startHeight + startY - moveEvent.clientY)),
      );
    };
    const finish = () => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', finish);
      window.removeEventListener('pointercancel', finish);
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', finish, { once: true });
    window.addEventListener('pointercancel', finish, { once: true });
  };
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
      className={`sidebar${collapsed ? ' sidebar--collapsed' : ''}${libraryExpanded ? ' sidebar--library-expanded' : ''}${libraryCollapsed ? ' sidebar--library-collapsed' : ''}`}
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
        <>
        {!libraryCollapsed && !libraryExpanded ? (
          <div
            className="library-sources__resize"
            role="separator"
            aria-label="Resize Library"
            aria-orientation="horizontal"
            onPointerDown={beginLibraryResize}
          />
        ) : null}
        <nav
          className="library-sources"
          aria-label="Library sources"
          style={
            libraryHeight === null || libraryCollapsed || libraryExpanded
              ? undefined
              : { height: `${libraryHeight}px`, maxHeight: 'none' }
          }
        >
          <div className="library-sources__heading">
            <span>Library</span>
            <div className="library-sources__actions">
              <button
                type="button"
                className="library-sources__refresh"
                onClick={() => {
                  setLibraryCollapsed((value) => !value);
                  setLibraryExpanded(false);
                  setLibraryHeight(null);
                }}
                aria-expanded={!libraryCollapsed}
              >
                {libraryCollapsed ? 'Show' : 'Hide'}
              </button>
              {!libraryCollapsed ? (
              <button
                type="button"
                className="library-sources__refresh"
                onClick={() => {
                  setLibraryExpanded((expanded) => !expanded);
                  setLibraryHeight(null);
                }}
                aria-expanded={libraryExpanded}
              >
                {libraryExpanded ? 'Restore' : 'Expand'}
              </button>
              ) : null}
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
          </div>
          {!libraryCollapsed ? (
          <>
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
                const sourceCollapsed = collapsedLibrarySources.has(member.membership_id);
                const sourceHasMore =
                  libraryHasMore && evidence.window_truncated && evidence.answer.next_cursor !== null;
                return (
                  <section className="library-sources__source" key={member.membership_id}>
                    <div className="library-sources__source-heading">
                      <button
                        type="button"
                        className="library-sources__source-toggle"
                        aria-expanded={!sourceCollapsed}
                        aria-label={`${sourceCollapsed ? 'Expand' : 'Collapse'} ${member.name}`}
                        onClick={() => {
                          setCollapsedLibrarySources((current) => {
                            const next = new Set(current);
                            if (next.has(member.membership_id)) {
                              next.delete(member.membership_id);
                            } else {
                              next.add(member.membership_id);
                            }
                            return next;
                          });
                        }}
                      >
                        <span className="library-sources__disclosure" aria-hidden="true">
                          {sourceCollapsed ? '›' : '⌄'}
                        </span>
                        {sourceIcon(member.kind)}
                        <strong>{member.name}</strong>
                      </button>
                      <small>
                        {member.kind === 'local-folder'
                          ? member.editable
                            ? member.active
                              ? 'Local · editable'
                              : 'Local · open to edit'
                            : 'Read-only'
                          : member.editable
                            ? 'Read & write'
                            : 'Read-only'}
                      </small>
                    </div>
                    {!sourceCollapsed ? (
                      <>
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
                        {sourceHasMore && onLoadMoreLibrary ? (
                          <button
                            type="button"
                            className="library-sources__refresh library-sources__load-more"
                            aria-label={`Load more ${member.name}`}
                            onClick={() => onLoadMoreLibrary(member.membership_id)}
                            disabled={libraryLoading}
                          >
                            {libraryLoading ? 'Reading…' : 'Load more'}
                          </button>
                        ) : null}
                      </>
                    ) : null}
                  </section>
                );
              }),
          )}
          {libraryEvidence.length === 0 && libraryLoading ? <p>Reading Library…</p> : null}
          </>
          ) : null}
        </nav>
        </>
      ) : null}

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
