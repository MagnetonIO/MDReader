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
  onRefresh?: () => void;
  onContinue?: () => void;
  onBrowse(): void;
  children?: ReactNode;
}

export function CatalogSourceRow({ source, expanded, busy, exactCopiesOnly, canManage,
  onRefresh, onContinue, onBrowse, children }: Props) {
  const needsActivation = source.coverage === 'needs_activation';
  return <div className="reader-catalog__source" role="group" aria-label={`Source ${source.name}`}>
    <div className="reader-catalog__source-header">
    <button type="button" className="reader-catalog__source-open"
      title={source.name}
      aria-label={`${needsActivation ? 'Activate' : 'Browse'} ${source.name}`}
      aria-expanded={needsActivation ? undefined : expanded}
      disabled={needsActivation ? !canManage || exactCopiesOnly : busy || !source.enabled || Boolean(source.failure)}
      onClick={onBrowse}>
      {source.kind === 'local' ? <FolderIcon /> : <ProviderIcon provider={source.kind} width={18} height={18} />}
      <span className="reader-catalog__source-label">
        <strong>{source.name}</strong>
        {!source.enabled || needsActivation || source.coverage === 'not_indexed' ? <small>{sourceLabel(source)}</small> : null}
      </span>
      <ChevronRight className="reader-catalog__chevron" />
    </button>
    {onRefresh ? <button type="button" className="reader-catalog__refresh" disabled={busy || !source.enabled || Boolean(source.failure)}
      aria-label={`Refresh ${source.name} documents`} title="Refresh documents" onClick={onRefresh}>
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
        <path d="M20 7v5h-5M4 17v-5h5" /><path d="M6 7a7 7 0 0 1 12-1l2 3M4 15l2 3a7 7 0 0 0 12-1" />
      </svg>
    </button> : null}
    </div>
    {source.failure ? <p role="status">{catalogErrorMessage(source.failure)}</p> : null}
    {expanded ? <div className="reader-catalog__source-children">{children}
      {onContinue ? <button type="button" className="reader-catalog__continue" disabled={busy || !source.enabled || Boolean(source.failure)}
        aria-label={`Continue loading ${source.name} documents`} onClick={onContinue}>Continue loading</button> : null}
    </div> : null}
  </div>;
}

function sourceLabel(source: CatalogSource): string {
  if (!source.enabled) return 'Paused';
  if (source.coverage === 'needs_activation') return 'Activate to browse';
  return 'Not indexed yet';
}
