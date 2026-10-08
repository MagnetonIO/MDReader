import { useEffect } from 'react';
import type { AccountBackend } from './backend.js';
import { useAccountSync } from './useAccountSync.js';
import { AccountJobDetails } from './AccountJobDetails.js';
import { AccountEnrollment } from './AccountEnrollment.js';

export function AccountSyncPanel({ backend, active = true }: { backend?: AccountBackend; active?: boolean }) {
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
        Sign in through the native browser, then explicitly enroll a library.
        Local documents and pending work remain on this device when disconnected.
      </p>
        <button type="button" disabled={account.busy} onClick={() => void account.refresh()}>
          Refresh account state
        </button>
        {account.error ? <p role="alert">{account.error}</p> : null}
        {!status ? (
          <p role="status">{account.busy ? 'Reading account state…' : 'Account state has not been confirmed.'}</p>
        ) : <>
          <p role="status">
            {!status.configured ? 'Encrypted sync is not configured in this build.'
              : status.connected ? 'Connected to the retained library.'
              : status.retainedLibrary ? 'Library retained; sign in to resume.'
              : 'Not connected. Select a library explicitly to enroll.'}
          </p>
          <div className="settings-account-actions">
            <button type="button" disabled={disabled || !status.retainedLibrary}
              onClick={() => void account.act({ action: 'sign_in' })}>Sign in / reconnect</button>
            <button type="button" disabled={disabled || !status.connected}
              onClick={() => void account.act({ action: 'sync_now' })}>Sync queued work</button>
            <button type="button" disabled={account.busy || !status.configured}
              onClick={() => void account.disconnect()}>Disconnect sync</button>
            {running ? (
              <button type="button" disabled={account.busy} onClick={() => void account.cancel()}>Cancel this operation</button>
            ) : null}
          </div>
          {status.custody ? (
            <p>
              Custody: {status.custody.state.replaceAll('_', ' ')}
              {status.custody.enrollmentUncertain ? ' · enrollment outcome uncertain' : ''}
              {status.custody.enrollmentExpired ? ' · enrollment expired' : ''}.
            </p>
          ) : <p>Native secure custody has not been inspected. This does not prove that keys are absent.</p>}
          <AccountJobDetails job={status.latestJob} />
          <AccountEnrollment status={status} disabled={disabled} act={account.act} />
        </>}
    </section>
  );
}
