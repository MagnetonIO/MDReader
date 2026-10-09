import { useEffect, useId, useRef } from 'react';
import type { useAccountSync } from '../useAccountSync';
import { pendingExplanation, type Choice } from './model';
import { usePendingChanges } from './usePendingChanges';
import './pending-changes.css';

export function PendingChanges({ account, active }: { account: ReturnType<typeof useAccountSync>; active: boolean }) {
  const pending = usePendingChanges(account, active);
  const heading = useId();
  if (!account.status?.configured || !account.status.retainedLibrary && !account.status.connected) return null;
  return <section className="settings-pending-changes" aria-labelledby={heading}>
    <header>
      <h4 id={heading}>Changes that need attention</h4>
      <button type="button" disabled={pending.busy || !pending.connected} onClick={pending.refresh}>Refresh changes</button>
    </header>
    {!pending.connected ? <p>Sign in to review changes for this library. Your local copies are kept.</p>
      : pending.entries === null ? <p role="status">{pending.busy ? 'Checking pending changes…' : 'Pending changes have not been checked.'}</p>
      : !pending.entries.length ? <p>No changes need review.</p>
      : pending.entries.map((entry) => <article key={`${entry.sequence}:${entry.revisionId}`} className="settings-pending-change">
        <strong>{entry.title || 'Untitled document'}</strong>
        <p>{pendingExplanation(entry.reason, entry.kind)}</p>
        {entry.choices.includes('keep_both') ? <>
          <p className="settings-pending-change__hint">Keep your document and add the incoming version as a separate copy.</p>
          <button type="button" className="settings-account-primary" disabled={pending.busy || !pending.connected || !!pending.attempt}
            onClick={() => pending.review(entry)}>Keep both copies…</button>
        </> : <p className="settings-pending-change__hint">This app cannot safely resolve this change yet. It stays pending; nothing is discarded.</p>}
        <details className="settings-pending-change__details"><summary>Change details</summary>
          <dl><dt>Change</dt><dd>{entry.sequence}</dd><dt>Revision</dt><dd>{entry.revisionId}</dd></dl>
        </details>
      </article>)}
    {pending.choice ? <KeepBoth choice={pending.choice} busy={pending.busy || !pending.connected} cancel={pending.cancel} confirm={pending.confirm} /> : null}
    {pending.attempt ? <p role="status">{pending.busy ? 'Keeping both copies…'
      : 'Your choice could not be confirmed. Refresh changes before choosing again; your local document is kept.'}</p> : null}
    {pending.notice ? <div className="settings-pending-changes__result">
      <p role="status">{pending.notice}</p>
      <button type="button" disabled={pending.busy || !pending.connected}
        onClick={() => void account.act({ action: 'sync_now' })}>Continue sync</button>
    </div> : null}
  </section>;
}

function KeepBoth({ choice, busy, cancel, confirm }: { choice: Choice; busy: boolean; cancel(): void; confirm(): void }) {
  const cancelButton = useRef<HTMLButtonElement>(null);
  const title = useId();
  useEffect(() => {
    const trigger = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    cancelButton.current?.focus();
    return () => { if (trigger?.isConnected) trigger.focus(); };
  }, []);
  return <div className="settings-pending-confirm" role="group" aria-labelledby={title}>
    <h5 id={title}>Keep both copies of “{choice.entry.title || 'Untitled document'}”?</h5>
    <p>Your current document, images, pin, and saved place stay unchanged. The incoming version is added to Library as a separate copy.</p>
    <div className="settings-account-actions">
      <button ref={cancelButton} type="button" onClick={cancel}>Cancel</button>
      <button type="button" className="settings-account-primary" disabled={busy} onClick={confirm}>Keep both copies</button>
    </div>
  </div>;
}
