/** The collapsible left rail: source switcher, virtualized file tree, recents.
 * A quiet rail — muted surface, thin borders — that never competes with the
 * reading column. */

import type { ListingFailure, WorkspaceFile } from '../platform/fs.js';
import type {
  CloudDocumentQueryResponse,
  CloudDocumentRow,
  CloudProvider,
  CloudSourceDto,
} from '../bindings.js';
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
  /** Why the listing failed, or `null`. Takes precedence over the file tree:
   *  a folder we could not read must never be drawn as a folder with no files. */
  listingError?: ListingFailure | null;
  onPickFolder: () => void;
  onOpenFile: (file: WorkspaceFile) => void;
  onOpenRecent: (ref: DocRef) => void;
  isIgnored?: (path: string) => boolean;
  cloudSources?: CloudSourceDto[];
  cloudDocuments?: Partial<Record<CloudProvider, CloudDocumentQueryResponse>>;
  cloudSearch?: string;
  cloudLoading?: boolean;
  cloudError?: string | null;
  onCloudSearch?: (value: string) => void;
  onOpenCloud?: (
    source: CloudSourceDto,
    response: CloudDocumentQueryResponse,
    row: CloudDocumentRow,
  ) => void;
  onRefreshCloud?: () => void;
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
  cloudSources = [],
  cloudDocuments = {},
  cloudSearch = '',
  cloudLoading = false,
  cloudError = null,
  onCloudSearch,
  onOpenCloud,
  onRefreshCloud,
}: SidebarProps) {
  const connectedSources = cloudSources.filter(
    (source) => source.connection !== 'disconnected',
  );
  const search = cloudSearch.trim().toLocaleLowerCase();
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

      {connectedSources.length > 0 ? (
        <nav className="cloud-library" aria-label="Connected Library sources">
          <div className="cloud-library__heading">
            <span>Connected</span>
            {onRefreshCloud ? (
              <button type="button" onClick={onRefreshCloud} disabled={cloudLoading}>
                {cloudLoading ? 'Reading…' : 'Refresh'}
              </button>
            ) : null}
          </div>
          {onCloudSearch ? (
            <input
              type="search"
              value={cloudSearch}
              onChange={(event) => onCloudSearch(event.currentTarget.value)}
              placeholder="Search connected documents"
              aria-label="Search connected Library documents"
            />
          ) : null}
          {cloudError ? <p className="cloud-library__error">{cloudError}</p> : null}
          {connectedSources.map((source) => {
            const response = cloudDocuments[source.provider];
            const rows =
              response?.rows.filter((row) => {
                if (!search) return true;
                const fields =
                  row.fields && typeof row.fields === 'object' && !Array.isArray(row.fields)
                    ? (row.fields as Record<string, unknown>)
                    : {};
                const title = typeof fields.title === 'string' ? fields.title : row.record_key;
                return title.toLocaleLowerCase().includes(search);
              }) ?? [];
            return (
              <section className="cloud-library__source" key={source.id}>
                <div>
                  <strong>{source.label}</strong>
                  <small>
                    {source.enabled ? 'Read-through · not locally indexed' : 'Paused in Settings'}
                  </small>
                </div>
                {source.connection === 'reauthorize' ? (
                  <p>Sign in again from Settings to read this source.</p>
                ) : !source.enabled ? null : response ? (
                  rows.length > 0 ? (
                    <ul>
                      {rows.map((row) => {
                        const fields =
                          row.fields &&
                          typeof row.fields === 'object' &&
                          !Array.isArray(row.fields)
                            ? (row.fields as Record<string, unknown>)
                            : {};
                        const title =
                          typeof fields.title === 'string' ? fields.title : row.record_key;
                        return (
                          <li key={`${row.source_id}:${row.record_key}`}>
                            <button
                              type="button"
                              aria-label={`Open ${title} from ${source.label}`}
                              onClick={() => onOpenCloud?.(source, response, row)}
                            >
                              {title}
                            </button>
                          </li>
                        );
                      })}
                    </ul>
                  ) : (
                    <p>{search ? 'No connected documents match.' : 'No documents found.'}</p>
                  )
                ) : (
                  <p>{cloudLoading ? 'Reading provider…' : 'Refresh to read this source.'}</p>
                )}
              </section>
            );
          })}
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
