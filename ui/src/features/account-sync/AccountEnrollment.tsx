import { useState, type Ref } from 'react';
import type { AccountAction, AccountStatus } from './backend.js';
import { DevicePairing } from './DevicePairing';
import { canReconcileEnrollment, canRenewEnrollment } from './pairing-view';

interface Props { status: AccountStatus; disabled: boolean; act(action: AccountAction): Promise<void>; disconnect(): Promise<void>; recoveryRef?: Ref<HTMLDetailsElement> }
const validLibrary = (value: string) => /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(value);
export function AccountEnrollment({ status, disabled, act, disconnect, recoveryRef }: Props) {
  const [remoteLibrary, setRemoteLibrary] = useState('');
  const [setupMode, setSetupMode] = useState<'new' | 'existing'>('new');
  const suggestedLibrary = status.existingLibraryId ?? '';
  const selectedLibrary = suggestedLibrary || status.activeLibraryId || remoteLibrary;
  const pairCandidate = status.custody?.state === 'pair_candidate';
  const initial = !status.retainedLibrary && !suggestedLibrary && !status.custody;
  return <div className="settings-account-tools">
    {initial ? <section className="account-guided-setup" aria-labelledby="account-first-setup"><h4 id="account-first-setup">Set up sync</h4>
      <p>Is this your first device, or do you already use MDReader sync?</p>
      <fieldset disabled={disabled}><legend>How would you like to connect?</legend>
        <label><input type="radio" name="account-setup-mode" checked={setupMode === 'new'} onChange={() => setSetupMode('new')} />This is my first device</label>
        <label><input type="radio" name="account-setup-mode" checked={setupMode === 'existing'} onChange={() => setSetupMode('existing')} />Connect an existing library</label>
      </fieldset>
      {setupMode === 'new' ? <><p>Sign in to create your encrypted library. Google or Dropbox connections are separate from device sync.</p>
        <button type="button" className="settings-account-primary" disabled={disabled} onClick={() => void act({ action: 'bootstrap', remoteLibrary: crypto.randomUUID() })}>Create sync library</button>
      </> : <><p>On your connected device, open Account &amp; sync → Advanced and copy its library ID. Then sign in here with the same account.</p>
        <details className="settings-account-advanced"><summary>Advanced · existing library ID</summary><label>Library ID<input value={remoteLibrary} maxLength={36} autoComplete="off" onChange={event => setRemoteLibrary(event.target.value)} /></label></details>
        <button type="button" className="settings-account-primary" disabled={disabled || !validLibrary(remoteLibrary)} onClick={() => void act({ action: 'join_device', remoteLibrary })}>Connect this device</button>
      </>}
      <p>Setup may ask you to confirm the same account a second time to authorize this device. Signing in does not upload documents.</p>
    </section> : null}
    {status.connected ? <DevicePairing key={`${status.activeLibraryId}:${status.selectedScope?.accountDigest}`} status={status} disabled={disabled} act={act} /> : null}
    <details ref={recoveryRef} className="settings-account-advanced"><summary>Recovery</summary>
      {!status.recoveryAvailable ? <p>Recovery dialogs are unavailable in this build. No recovery secret is accepted by the reader UI.</p> : <p>Use a recovery file and its separate secret when a connected device is unavailable. Recovery happens in native dialogs.</p>}
      {status.custody?.recoveryExportAvailable ? <section><h4>Back up access to your library</h4><ol className="account-guided-steps"><li>Save the encrypted recovery file.</li><li>Store its separately presented secret somewhere else. Anyone with both can recover your library.</li></ol>
        <button type="button" disabled={disabled || !status.recoveryAvailable} onClick={() => void act({ action: 'export_recovery' })}>Save recovery file</button>
      </section> : null}
      {!status.connected && pairCandidate ? <section><h4>Recovery on another device</h4>
        <p>This pending connection cannot switch to recovery in this build. Keep this device’s local library.</p>
        <ol className="account-guided-steps"><li>On a connected device, open Account &amp; sync → Recovery and save a recovery file.</li><li>On another device, open Recovery before starting a device request. Use that file and its separately stored secret.</li></ol>
        {!status.recoveryAvailable ? <p>Use a build with native recovery dialogs.</p> : null}
      </section> : null}
      {!status.connected && !pairCandidate ? <section><h4>Restore access on this device</h4>
        {suggestedLibrary || status.activeLibraryId ? <p>Recovery uses the library already selected on this device.</p> : <details className="settings-account-advanced"><summary>Advanced · recovery library ID</summary><label>Library ID for recovery<input value={remoteLibrary} maxLength={36} autoComplete="off" onChange={event => setRemoteLibrary(event.target.value)} /></label></details>}
        <ol className="account-guided-steps"><li>Sign in with the same account that owns the library.</li><li>Choose the encrypted recovery file, then enter its secret in the native dialog.</li></ol>
        <button type="button" disabled={disabled || !validLibrary(selectedLibrary) || !status.recoveryAvailable || status.existingLibrarySameSelection} onClick={() => void act({ action: 'recover', remoteLibrary: selectedLibrary })}>Recover this device</button>
      </section> : null}
      {!status.custody?.recoveryExportAvailable ? <button type="button" disabled>Save recovery file</button> : null}
    </details>
    <details className="settings-account-advanced"><summary>Advanced</summary>
      <p>Library ID: {status.activeLibraryId || suggestedLibrary || 'No library selected'}</p>
      {status.selectedScope ? <><p>Account fingerprint: {status.selectedScope.accountDigest}</p><p>Device fingerprint: {status.selectedScope.deviceId}</p></> : null}
      {status.custody ? <p>Secure storage: {status.custody.state.replaceAll('_', ' ')}{status.custody.enrollmentUncertain ? ' · enrollment outcome uncertain' : ''}{status.custody.enrollmentExpired ? ' · enrollment expired' : ''}.</p> : <p>Secure storage has not been checked. Existing keys may still be present.</p>}
      <p>These actions keep the selected library and existing keys.</p>
      {status.custody?.state === 'pair_candidate' && !status.custody.materialStaged && status.custody.enrollmentUncertain ? <p>A device request does not complete enrollment. Finish approval above; checking enrollment is available after approval is saved.</p> : <p>Refresh status before retrying an uncertain operation.</p>}
      <div className="settings-account-actions">
        {status.connected || status.retainedLibrary ? <button type="button" disabled={disabled} onClick={() => void disconnect()}>Disconnect sync</button> : null}
        <button type="button" disabled={disabled || !canReconcileEnrollment(status)} onClick={() => void act({ action: 'reconcile_enrollment' })}>Check uncertain enrollment</button>
        <button type="button" disabled={disabled || !canRenewEnrollment(status)} onClick={() => void act({ action: 'renew_enrollment' })}>Retry enrollment sign-in</button>
        <button type="button" disabled={disabled || !status.retainedLibrary} onClick={() => void act({ action: 'retry_custody_write' })}>Retry secure storage</button>
        <button type="button" disabled={disabled || !status.connected || status.custody?.deviceApproval !== 'prepared' && status.custody?.deviceApproval !== 'uncertain'} onClick={() => void act({ action: 'retry_device_approval' })}>Check uncertain device approval</button>
      </div>
    </details>
  </div>;
}
