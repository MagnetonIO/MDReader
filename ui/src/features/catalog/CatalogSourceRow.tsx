import type { ReactNode } from 'react';
import { ChevronRight, FolderIcon } from '../../shell/icons';
import { ProviderIcon } from '../providers/ProviderIcon';
import { catalogErrorMessage, type CatalogSource } from './backend';

interface Props {
  source: CatalogSource;
  expanded: boolean;
  busy: boolean;
  exactCopiesOnly: boolean;
  canManage: boolean;
  searchEnabled: boolean;
  readerSourceId?: string;
  selectedProviderSources: readonly string[];
  onSelectSearch(id: string, checked: boolean): void;
  onBrowse(): void;
  children?: ReactNode;
}

export function CatalogSourceRow({ source, expanded, busy, exactCopiesOnly, canManage,
  searchEnabled, readerSourceId, selectedProviderSources, onSelectSearch, onBrowse, children }: Props) {
  const needsActivation = source.coverage === 'needs_activation';
  const cloud = source.catalogId.startsWith('remote:');
  return <div className="reader-catalog__source" role="group" aria-label={`Source ${source.name}`}>
    <button type="button" className="reader-catalog__source-open"
      aria-label={`${needsActivation ? 'Activate' : 'Browse'} ${source.name}`}
      aria-expanded={needsActivation ? undefined : expanded}
      disabled={needsActivation ? !canManage || exactCopiesOnly : busy || !source.enabled || Boolean(source.failure)}
      onClick={onBrowse}>
      {source.kind === 'local' ? <FolderIcon /> : <ProviderIcon provider={source.kind} width={18} height={18} />}
      <span className="reader-catalog__source-label">
        <strong>{source.name}</strong>
        <small>{sourceLabel(source)}</small>
      </span>
      <ChevronRight className="reader-catalog__chevron" />
    </button>
    {searchEnabled && cloud ? readerSourceId ? (
      <label className="reader-catalog__search-selection">
        <input type="checkbox" aria-label={`Include ${source.name} in search`}
          checked={selectedProviderSources.includes(readerSourceId)} disabled={!source.enabled}
          onChange={(event) => onSelectSearch(readerSourceId, event.currentTarget.checked)} />
        Include in search
      </label>
    ) : source.enabled ? (
      <div className="reader-catalog__search-hint">
        {expanded ? <small>Open a file to enable search.</small> : (
          <button type="button" disabled={busy || Boolean(source.failure)} onClick={onBrowse}>
            Open a file to enable search
          </button>
        )}
      </div>
    ) : null : null}
    {source.failure ? <p role="status">{catalogErrorMessage(source.failure)}</p> : null}
    {expanded ? <div className="reader-catalog__source-children">{children}</div> : null}
  </div>;
}

function sourceLabel(source: CatalogSource): string {
  if (!source.enabled) return 'Paused';
  if (source.coverage === 'needs_activation') return 'Activate to browse';
  if (source.kind === 'capture') return 'Captured notes';
  if (source.kind === 'device') return 'Saved documents';
  if (source.kind === 'local') return source.coverage === 'not_indexed' ? 'Not indexed yet' : 'Folder';
  const provider = source.kind === 'dropbox' ? 'Dropbox' : source.kind === 'google_drive' ? 'Google Drive' : 'Cloud source';
  return source.coverage === 'never_indexed' ? `${provider} · Connected` : provider;
}
