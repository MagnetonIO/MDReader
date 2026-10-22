import { useEffect, useId, useRef, useState, type ReactNode } from 'react';
import type { ProviderSource } from '../../platform/providers';
import { ActionMenu, type ActionMenuItem } from './ActionMenu';
import { ProviderSourceSummary } from './ProviderSourceSummary';
import { canBrowseSource, providerName, sourceStatus } from './source-presentation';

export interface SourceRowActions {
  onBrowse(): void;
  onRefresh(): void;
  onReconnect(): void;
  onPause(): void;
  onDisconnect(): void;
  onChooseFolder?(): void;
  /** Resolves null when unknown; the confirm waits so Disconnect never races the check. */
  checkPendingChanges?(): Promise<boolean | null>;
}

/** One connected source: identity and status, a primary action and the remaining actions in a menu. */
export function SourceRow({ source, allowEditing, busy, canReconnect, folderOpen = false, actions, search, children }: {
  source: ProviderSource; allowEditing: boolean; busy: boolean; canReconnect: boolean; folderOpen?: boolean;
  actions: SourceRowActions; search?: ReactNode; children?: ReactNode;
}) {
  const [confirming, setConfirming] = useState(false);
  const more = useRef<HTMLButtonElement>(null);
  function closeConfirm() { setConfirming(false); more.current?.focus(); }
  const status = sourceStatus(source);
  const prominentReconnect = status === 'paused' || status === 'reconnect';
  const items: ActionMenuItem[] = [
    { id: 'refresh', label: 'Refresh file list', disabled: busy || !source.enabled || !source.capabilities.list, onSelect: actions.onRefresh },
    ...actions.onChooseFolder ? [{ id: 'folder', label: folderOpen ? 'Hide Drive folders' : 'Choose a Drive folder', disabled: busy, onSelect: actions.onChooseFolder }] : [],
    ...prominentReconnect ? [] : [{ id: 'reconnect', label: 'Reconnect', disabled: busy || !canReconnect, onSelect: actions.onReconnect }],
    { id: 'pause', label: 'Pause', disabled: busy || !source.enabled, onSelect: actions.onPause },
    { id: 'disconnect', label: 'Disconnect…', danger: true, separated: true, disabled: busy, onSelect: () => setConfirming(true) },
  ];
  return <li className="provider-source" aria-label={source.name}>
    <div className="provider-source__main">
      <ProviderSourceSummary source={source} allowEditing={allowEditing} />
      <div className="provider-source__actions">
        {prominentReconnect ? <button type="button" className="provider-button--primary" disabled={busy || !canReconnect} onClick={actions.onReconnect}>Reconnect</button> : null}
        {canBrowseSource(source) ? <button type="button" className={prominentReconnect ? undefined : 'provider-button--primary'} disabled={busy} onClick={actions.onBrowse}>Browse</button> : null}
        <ActionMenu label={`More actions for ${source.name}`} icon={<MoreIcon />} items={items} triggerRef={more} />
      </div>
    </div>
    {search}
    {confirming ? <DisconnectConfirm source={source} busy={busy} checkPendingChanges={actions.checkPendingChanges} onCancel={closeConfirm}
      onConfirm={() => { closeConfirm(); actions.onDisconnect(); }} /> : null}
    {children}
  </li>;
}

function DisconnectConfirm({ source, busy, checkPendingChanges, onConfirm, onCancel }: {
  source: ProviderSource; busy: boolean; checkPendingChanges?(): Promise<boolean | null>; onConfirm(): void; onCancel(): void;
}) {
  const titleId = useId();
  const cancel = useRef<HTMLButtonElement>(null);
  const [pending, setPending] = useState<boolean | null | 'checking'>(checkPendingChanges ? 'checking' : null);
  // Checked once per opened confirm; the owner passes a fresh callback each render.
  const check = useRef(checkPendingChanges);
  useEffect(() => { cancel.current?.focus(); }, []);
  useEffect(() => {
    if (!check.current) return;
    let current = true;
    void check.current().then((value) => { if (current) setPending(value); });
    return () => { current = false; };
  }, []);
  return <div className="provider-confirm" role="group" aria-labelledby={titleId}
    onKeyDown={event => { if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); onCancel(); } }}>
    <p id={titleId}><strong>Disconnect {source.name}?</strong> Removes this account from MDReader, deletes downloaded copies and the saved sign-in. Files in {providerName(source.kind)} are not touched.</p>
    {pending === true ? <p>Unsaved changes stay in draft recovery, but can't be checked against {providerName(source.kind)} after disconnecting.</p> : null}
    <div className="provider-confirm__actions">
      <button type="button" className="provider-button--danger" disabled={busy || pending === 'checking'} onClick={onConfirm}>Disconnect</button>
      <button type="button" ref={cancel} onClick={onCancel}>Cancel</button>
    </div>
  </div>;
}

function MoreIcon() {
  return <svg width="18" height="18" viewBox="0 0 24 24" aria-hidden="true" focusable="false" fill="currentColor">
    <circle cx="5" cy="12" r="1.8" /><circle cx="12" cy="12" r="1.8" /><circle cx="19" cy="12" r="1.8" />
  </svg>;
}
