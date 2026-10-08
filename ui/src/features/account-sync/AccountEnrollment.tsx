import { useState } from 'react';
import type { AccountAction, AccountStatus } from './backend.js';

interface Props {
  status: AccountStatus;
  disabled: boolean;
  act(action: AccountAction): Promise<void>;
}
export function AccountEnrollment({ status, disabled, act }: Props) {
  const [remoteLibrary, setRemoteLibrary] = useState('');
  const [packet, setPacket] = useState('');
  return (
    <details className="settings-account-advanced">
      <summary>Set up sync</summary>
      <p>
        Set up a first device or connect to your existing library. Use its exact library ID.
        Do not create a replacement library when retained keys are missing or uncertain.
      </p>
      <label>
        Library ID
        <input value={remoteLibrary} maxLength={36} autoComplete="off"
          onChange={(event) => setRemoteLibrary(event.target.value)} />
      </label>
      <details>
        <summary>Devices</summary>
        <div className="settings-account-actions">
          <button type="button" disabled={disabled || status.retainedLibrary}
            onClick={() => setRemoteLibrary(crypto.randomUUID())}>Generate new library ID</button>
          <button type="button" disabled={disabled || !remoteLibrary || status.retainedLibrary}
            onClick={() => void act({ action: 'bootstrap', remoteLibrary })}>Set up first device</button>
          <button type="button" disabled={disabled || !remoteLibrary}
            onClick={() => void act({ action: 'join_device', remoteLibrary })}>Connect this device</button>
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
      <details>
        <summary>Recovery</summary>
        {!status.recoveryAvailable ? <p>Recovery dialogs are unavailable in this build. No recovery secret is accepted by the reader UI.</p> : (
          <p>Choose and save recovery material through native dialogs. Keep the file and its separately presented secret in different safe places.</p>
        )}
        <div className="settings-account-actions">
          <button type="button" disabled={disabled || !remoteLibrary || !status.recoveryAvailable}
            onClick={() => void act({ action: 'recover', remoteLibrary })}>Recover this device</button>
          <button type="button" disabled={disabled || !status.recoveryAvailable || !status.custody?.recoveryExportAvailable}
            onClick={() => void act({ action: 'export_recovery' })}>Save recovery file</button>
        </div>
      </details>
      <details>
        <summary>Troubleshooting</summary>
        <p>Refresh account state first. These actions retain the same library and existing custody rather than creating replacement keys.</p>
        <div className="settings-account-actions">
          <button type="button" disabled={disabled || !status.retainedLibrary}
            onClick={() => void act({ action: 'reconcile_enrollment' })}>Check uncertain enrollment</button>
          <button type="button" disabled={disabled || !status.retainedLibrary}
            onClick={() => void act({ action: 'renew_enrollment' })}>Renew enrollment sign-in</button>
          <button type="button" disabled={disabled || !status.retainedLibrary}
            onClick={() => void act({ action: 'retry_custody_write' })}>Retry secure storage</button>
          <button type="button" disabled={disabled || !status.retainedLibrary}
            onClick={() => void act({ action: 'retry_device_approval' })}>Check uncertain device approval</button>
        </div>
      </details>
      <p>Signing in does not select documents for upload. Recovery secrets stay in native UI.</p>
    </details>
  );
}
