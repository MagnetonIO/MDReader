import { useState } from 'react';
import type { AccountAction, AccountStatus } from './backend.js';

interface Props {
  status: AccountStatus;
  disabled: boolean;
  act(action: AccountAction): Promise<void>;
}
export function AccountEnrollment({ status, disabled, act }: Props) {
  const [remoteLibrary, setRemoteLibrary] = useState('');
  const suggestedLibrary = status.existingLibraryId ?? '';
  const selectedLibrary = suggestedLibrary || status.activeLibraryId || remoteLibrary;
  const [packet, setPacket] = useState('');
  const [setupMode, setSetupMode] = useState(status.retainedLibrary ? 'existing' : 'new');
  const connectionMode = status.retainedLibrary ? 'existing' : setupMode;
  const setupPending = status.custody?.state === 'bootstrap_pending';
  const connectionPending = status.custody?.state === 'pair_candidate' || status.custody?.state === 'recovery_candidate';
  return (
    <div className="settings-account-tools">
    {(!setupPending && !connectionPending) || suggestedLibrary ? <details className="settings-account-advanced" open={!status.connected && (!status.retainedLibrary || Boolean(suggestedLibrary))}>
      <summary>Set up sync</summary>
      {suggestedLibrary ? <>
        <p>{status.existingLibrarySameSelection
          ? 'This library is already set up on your account. Check setup above to confirm this device’s connection.'
          : 'Choose Connect this device above to use your existing library. Approve it from another device, or use Recovery below.'}</p>
        <details><summary>Library details</summary><p>Library ID: {suggestedLibrary}</p></details>
      </> : <>
        <p>Create a library for your first device, or connect to one you already use.</p>
        <label>
          How would you like to connect?
          <select value={connectionMode} disabled={disabled} onChange={(event) => setSetupMode(event.target.value)}>
            <option value="new" disabled={status.retainedLibrary}>This is my first device</option>
            <option value="existing">Connect to an existing library</option>
          </select>
        </label>
        {connectionMode === 'new' ? <>
          <p>Use this only if you have not set up sync on this account before.</p>
          <div className="settings-account-actions">
            <button type="button" className="settings-account-primary" disabled={disabled || status.retainedLibrary}
              onClick={() => void act({ action: 'bootstrap', remoteLibrary: crypto.randomUUID() })}>Create sync library</button>
          </div>
        </> : <>
          <p>Find the library ID in the sync details on your existing device. Keep the same ID when reconnecting or recovering.</p>
          <label>Library ID
            <input value={remoteLibrary} maxLength={36} autoComplete="off"
              onChange={(event) => setRemoteLibrary(event.target.value)} />
          </label>
          <div className="settings-account-actions">
            <button type="button" disabled={disabled || !remoteLibrary}
              onClick={() => void act({ action: 'join_device', remoteLibrary })}>Connect this device</button>
          </div>
        </>}
      </>}
      <p>Signing in does not select documents for upload.</p>
    </details> : null}
    <details className="settings-account-advanced">
        <summary>Connect another device</summary>
        <p>Exchange a public request and approval with your other device through a channel you trust.</p>
        <div className="settings-account-actions">
          <button type="button" disabled={disabled || !status.retainedLibrary}
            onClick={() => void act({ action: 'show_pair_request' })}>Show public device request</button>
        </div>
        <label>
          Public device request or approval
          <textarea value={packet} maxLength={8192} onChange={(event) => setPacket(event.target.value)} />
        </label>
        <div className="settings-account-actions">
          <button type="button" disabled={disabled || !packet}
            onClick={() => void act({ action: 'accept_device_approval', packet })}>Accept approval for this device</button>
          <button type="button" disabled={disabled || !packet}
            onClick={() => void act({ action: 'approve_device', packet })}>Approve requested device</button>
          <button type="button" disabled={disabled || status.custody?.deviceApproval !== 'confirmed'}
            onClick={() => void act({ action: 'device_approval_handed_off' })}>Confirm approval handed off</button>
        </div>
        <p>These packets are public pairing material. Never paste a recovery secret here.</p>
      </details>
      <details className="settings-account-advanced">
        <summary>Recovery</summary>
        {suggestedLibrary || (connectionPending && status.activeLibraryId) ? <p>Recovery uses the retained library.</p> : <label>Library ID for recovery
          <input value={remoteLibrary} maxLength={36} autoComplete="off"
            onChange={(event) => setRemoteLibrary(event.target.value)} />
        </label>}
        {!status.recoveryAvailable ? <p>Recovery dialogs are unavailable in this build. No recovery secret is accepted by the reader UI.</p> : (
          <p>Choose and save recovery material through native dialogs. Keep the file and its separately presented secret in different safe places.</p>
        )}
        <div className="settings-account-actions">
          <button type="button" disabled={disabled || !selectedLibrary || !status.recoveryAvailable || status.existingLibrarySameSelection}
            onClick={() => void act({ action: 'recover', remoteLibrary: selectedLibrary })}>Recover this device</button>
          <button type="button" disabled={disabled || !status.recoveryAvailable || !status.custody?.recoveryExportAvailable}
            onClick={() => void act({ action: 'export_recovery' })}>Save recovery file</button>
        </div>
      </details>
      <details className="settings-account-advanced">
        <summary>Troubleshooting</summary>
        <p>Refresh account state first. These actions retain the same library and existing custody rather than creating replacement keys.</p>
        <div className="settings-account-actions">
          <button type="button" disabled={disabled || !status.retainedLibrary}
            onClick={() => void act({ action: 'reconcile_enrollment' })}>Check uncertain enrollment</button>
          <button type="button" disabled={disabled || !status.retainedLibrary}
            onClick={() => void act({ action: 'renew_enrollment' })}>Retry enrollment sign-in</button>
          <button type="button" disabled={disabled || !status.retainedLibrary}
            onClick={() => void act({ action: 'retry_custody_write' })}>Retry secure storage</button>
          <button type="button" disabled={disabled || !status.retainedLibrary}
            onClick={() => void act({ action: 'retry_device_approval' })}>Check uncertain device approval</button>
        </div>
      </details>
    </div>
  );
}
