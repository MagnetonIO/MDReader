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
      <summary>Enrollment, device pairing &amp; recovery</summary>
      <p>
        Use the exact remote library UUID. Bootstrap creates a first-device library;
        pairing and recovery require the existing library. Never bootstrap to replace
        missing or uncertain retained keys.
      </p>
      <label>
        Remote library ID
        <input value={remoteLibrary} maxLength={36} autoComplete="off"
          onChange={(event) => setRemoteLibrary(event.target.value)} />
      </label>
      <div className="settings-account-actions">
        <button type="button" disabled={disabled || status.retainedLibrary}
          onClick={() => setRemoteLibrary(crypto.randomUUID())}>Generate new library ID</button>
        <button type="button" disabled={disabled || !remoteLibrary || status.retainedLibrary}
          onClick={() => void act({ action: 'bootstrap', remoteLibrary })}>Create first-device enrollment</button>
        <button type="button" disabled={disabled || !remoteLibrary}
          onClick={() => void act({ action: 'join_device', remoteLibrary })}>Pair this device</button>
        <button type="button" disabled={disabled || !remoteLibrary}
          onClick={() => void act({ action: 'recover', remoteLibrary })}>Recover with native file &amp; secret</button>
        <button type="button" disabled={disabled || !status.retainedLibrary}
          onClick={() => void act({ action: 'reconcile_enrollment' })}>Reconcile uncertain enrollment</button>
        <button type="button" disabled={disabled || !status.retainedLibrary}
          onClick={() => void act({ action: 'renew_enrollment' })}>Renew enrollment sign-in</button>
        <button type="button" disabled={disabled || !status.retainedLibrary}
          onClick={() => void act({ action: 'retry_custody_write' })}>Retry native custody write</button>
        <button type="button" disabled={disabled || !status.custody?.recoveryExportAvailable}
          onClick={() => void act({ action: 'export_recovery' })}>Export recovery through native dialog</button>
        <button type="button" disabled={disabled || !status.retainedLibrary}
          onClick={() => void act({ action: 'show_pair_request' })}>Show public pair request</button>
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
        <button type="button" disabled={disabled || !status.retainedLibrary}
          onClick={() => void act({ action: 'retry_device_approval' })}>Retry uncertain device approval</button>
        <button type="button" disabled={disabled || status.custody?.deviceApproval !== 'confirmed'}
          onClick={() => void act({ action: 'device_approval_handed_off' })}>Confirm public approval handed off</button>
      </div>
      <p>
        Recovery files and secrets are selected and presented only by native UI.
        This form accepts public pairing packets, never recovery secrets.
        No document is selected for upload by signing in.
      </p>
    </details>
  );
}
