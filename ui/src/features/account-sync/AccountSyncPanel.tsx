import { useEffect, type Ref } from 'react';
import type { AccountBackend } from './backend.js';
import { useAccountSync } from './useAccountSync.js';
import { AccountJobDetails } from './AccountJobDetails.js';
import { primaryAccountAction } from './primary-action';
import { AccountEnrollment } from './AccountEnrollment.js';
import type { DeviceReference } from '../device-reader/backend';
import { SelectedDocumentSync } from './SelectedDocumentSync';
import { PendingChanges } from './conflicts/PendingChanges';
import './account-sync.css';

export function AccountSyncPanel({ backend, active = true, selectedDocument }: { backend?: AccountBackend; active?: boolean; selectedDocument?: DeviceReference }) {
  const account = useAccountSync(backend, active);
  return <AccountSyncView account={account} active={active} selectedDocument={selectedDocument} />;
}

/** Mobile retains one observer across screens; desktop owns it in Settings. */
export function AccountSyncView({ account, active = true, refreshOnOpen = true, selectedDocument, headingRef }: {
  account: ReturnType<typeof useAccountSync>; active?: boolean; refreshOnOpen?: boolean; selectedDocument?: DeviceReference; headingRef?: Ref<HTMLHeadingElement>;
}) {
  useEffect(() => {
    if (active && refreshOnOpen) void account.refresh();
  }, [active, refreshOnOpen, account.refresh]);
  const status = account.status;
  const running = account.running || status?.latestJob?.state.state === 'running';
  const disabled = account.busy || running || !status?.configured;
  const setupPending = status?.custody?.state === 'bootstrap_pending';
  const existingFound = Boolean(status?.existingLibraryId);
  const primary = status ? primaryAccountAction(status) : null;
  const latestOutcome = status?.latestJob?.state.state === 'completed' ? status.latestJob.state.outcome : null;
  const requestReady = status?.custody?.state === 'pair_candidate' && latestOutcome?.kind === 'public_packet' && latestOutcome.packetKind === 'request';

  return (
    <section className="settings-section" aria-labelledby="settings-device-sync">
      <h3 id="settings-device-sync" ref={headingRef} tabIndex={headingRef ? -1 : undefined}>Account &amp; sync</h3>
      <p>
        Sync selected documents across your devices. Disconnecting keeps local copies.
      </p>
        {account.error ? <p role="alert">{account.error}</p> : null}
        {!status ? (
          <p role="status">{account.busy ? 'Reading account state…' : 'Account state has not been confirmed.'}</p>
        ) : <>
          <div className="settings-sync-status" role="group" aria-label="Sync connection">
          <span className="settings-sync-status__label">{!status.configured ? 'Sync unavailable' : status.connected ? 'Connected' : running ? 'Connecting…' : 'Not connected'}</span>
          <p role="status">
            {!status.configured ? 'Encrypted sync is not configured in this build.'
              : status.connected ? 'Connected to the retained library.'
              : existingFound ? status.existingLibrarySameSelection
                ? 'Setup needs confirmation. Check this device’s connection.'
                : 'Your existing sync library was found. Connect this device using approval or recovery.'
              : setupPending ? 'Setup is unfinished. Retry setup to confirm your library; existing keys are kept.'
              : status.custody?.state === 'pair_candidate' ? status.custody.enrollmentExpired
                ? 'The device request expired. Its identity and local copies remain retained.'
                : 'Connection is pending. Continue with approval from your existing device.'
              : status.custody?.state === 'recovery_candidate' ? 'Recovery is unfinished. Continue using your retained setup.'
              : status.retainedLibrary ? 'Library retained; sign in to resume.'
              : 'Set up sync to connect your devices. Signing in does not upload documents.'}
          </p>
          {!status.configured ? <p>Local reading and search remain available without an account.</p> : null}
          <div className="settings-account-actions">
            {status.configured && !status.connected && status.retainedLibrary && primary && !requestReady ?
              <button type="button" className="settings-account-primary" disabled={disabled}
                onClick={() => void account.act(primary.action)}>{primary.label}</button> : null}
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
          {status.configured && status.connected ? <>
            <PendingChanges account={account} active={active} />
            <SelectedDocumentSync account={account} selected={selectedDocument} />
          </> : null}
          <AccountJobDetails job={status.latestJob} />
          {status.configured ? <AccountEnrollment key={`${status.activeLibraryId}:${status.existingLibraryId}:${status.selectedScope?.accountDigest}`} status={status} disabled={disabled} act={account.act} /> : null}
        </>}
    </section>
  );
}
