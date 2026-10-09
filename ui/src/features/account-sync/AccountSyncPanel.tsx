import { useEffect } from 'react';
import type { AccountBackend } from './backend.js';
import { useAccountSync } from './useAccountSync.js';
import { AccountJobDetails } from './AccountJobDetails.js';
import { AccountEnrollment } from './AccountEnrollment.js';
import type { DeviceReference } from '../device-reader/backend';
import { SelectedDocumentSync } from './SelectedDocumentSync';
import { PendingChanges } from './conflicts/PendingChanges';

export function AccountSyncPanel({ backend, active = true, selectedDocument }: { backend?: AccountBackend; active?: boolean; selectedDocument?: DeviceReference }) {
  const account = useAccountSync(backend, active);
  useEffect(() => {
    if (active) void account.refresh();
  }, [active, account.refresh]);
  const status = account.status;
  const running = account.running || status?.latestJob?.state.state === 'running';
  const disabled = account.busy || running || !status?.configured;

  return (
    <section className="settings-section" aria-labelledby="settings-device-sync">
      <h3 id="settings-device-sync">Encrypted device sync</h3>
      <p>
        Sync selected documents across your devices. Disconnecting keeps local copies.
      </p>
        {account.error ? <p role="alert">{account.error}</p> : null}
        {!status ? (
          <p role="status">{account.busy ? 'Reading account state…' : 'Account state has not been confirmed.'}</p>
        ) : <>
          <div className="settings-sync-status">
          <span className="settings-sync-status__label">{!status.configured ? 'Unavailable' : status.connected ? 'Connected' : 'Not connected'}</span>
          <p role="status">
            {!status.configured ? 'Encrypted sync is not configured in this build.'
              : status.connected ? 'Connected to the retained library.'
              : status.retainedLibrary ? 'Library retained; sign in to resume.'
              : 'Not connected. Select a library explicitly to enroll.'}
          </p>
          {!status.configured ? <p>Local reading and search remain available without an account.</p> : null}
          <div className="settings-account-actions">
            {!status.connected && status.retainedLibrary ? <button type="button" className="settings-account-primary" disabled={disabled}
              onClick={() => void account.act({ action: 'sign_in' })}>Sign in / reconnect</button> : null}
            {status.connected ? <button type="button" className="settings-account-primary" disabled={disabled}
              onClick={() => void account.act({ action: 'sync_now' })}>Sync queued work</button> : null}
            <button type="button" disabled={account.busy} onClick={() => void account.refresh()}>Refresh status</button>
            {status.configured && (status.connected || status.retainedLibrary) ? <button type="button" disabled={account.busy}
              onClick={() => void account.disconnect()}>Disconnect sync</button>
            : null}
            {running ? (
              <button type="button" disabled={account.busy} onClick={() => void account.cancel()}>Cancel this operation</button>
            ) : null}
          </div>
          </div>
          <PendingChanges account={account} active={active} />
          <SelectedDocumentSync account={account} selected={selectedDocument} />
          {status.configured ? <details className="settings-account-advanced">
            <summary>Security details</summary>
            {status.custody ? (
            <p>
              Secure storage: {status.custody.state.replaceAll('_', ' ')}
              {status.custody.enrollmentUncertain ? ' · enrollment outcome uncertain' : ''}
              {status.custody.enrollmentExpired ? ' · enrollment expired' : ''}.
            </p>
            ) : <p>Secure storage has not been checked. Existing keys may still be present.</p>}
          </details> : null}
          <AccountJobDetails job={status.latestJob} />
          {status.configured ? <AccountEnrollment status={status} disabled={disabled} act={account.act} /> : null}
        </>}
    </section>
  );
}
