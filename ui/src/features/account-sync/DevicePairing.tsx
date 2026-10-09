import { useEffect, useRef, useState } from 'react';
import type { AccountAction, AccountStatus } from './backend';
import { parsePublicPacket, type PublicPairPacket } from './public-packet';
import { PublicPacketIdentity } from './PublicPacketIdentity';

export function DevicePairing({ status, disabled, act }: {
  status: AccountStatus; disabled: boolean; act(action: AccountAction): Promise<void>;
}) {
  const [text, setText] = useState('');
  const [packet, setPacket] = useState<PublicPairPacket | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [confirmed, setConfirmed] = useState(false);
  const [reading, setReading] = useState(false);
  const epoch = useRef(0);
  const approving = status.connected;
  const expected = approving ? 'mdreader-pair-request' : 'mdreader-pair-approval';
  const owner = `${status.activeLibraryId}:${status.selectedScope?.accountDigest}:${approving}`;
  useEffect(() => { epoch.current++; setText(''); setPacket(null); setError(null); setConfirmed(false); setReading(false); return () => { epoch.current++; }; }, [owner]);
  function accept(value: string, pasted = false) {
    setText(pasted ? value : ''); setConfirmed(false); setPacket(null); setError(null);
    try {
      const parsed = parsePublicPacket(value);
      if (parsed.kind !== expected) throw new Error(approving ? 'Choose the request file from the new device.' : 'Choose the approval file from your connected device.');
      if (status.activeLibraryId && parsed.libraryId !== status.activeLibraryId || status.selectedScope && (parsed.account !== status.selectedScope.accountDigest || parsed.libraryId !== status.selectedScope.libraryId)) throw new Error('This file belongs to another account or library. Your current connection is kept.');
      setText(value); setPacket(parsed);
    } catch (cause) { setError(cause instanceof Error ? cause.message : 'The public device file could not be read.'); }
  }
  async function choose(file: File | undefined) {
    if (!file || disabled) return;
    const request = ++epoch.current; setReading(true); setError(null); setConfirmed(false); setPacket(null); setText('');
    try {
      if (file.size > 8192) throw new Error('Choose a public device JSON file no larger than 8 KiB.');
      const value = await file.text(); if (request === epoch.current) accept(value);
    } catch (cause) { if (request === epoch.current) setError(cause instanceof Error ? cause.message : 'The public device file could not be read.'); }
    finally { if (request === epoch.current) setReading(false); }
  }
  return <details className="settings-account-advanced" open={!approving && status.custody?.state === 'pair_candidate'}>
    <summary>{approving ? 'Add another device' : 'Finish connecting this device'}</summary>
    <ol className="account-guided-steps">
      <li>{approving ? 'On your new device, choose Connect an existing library and save its device request file.' : 'Save the device request shown below and open it on a device that is already connected.'}</li>
      <li>{approving ? 'Choose that request here. Compare the full identities on both devices before approving.' : 'On the connected device, open Add another device, choose your request, and save the approval file.'}</li>
      <li>{approving ? 'Send the approval file back through a trusted channel. Keep this device open until the handoff is confirmed.' : 'Choose the approval here. Compare the full identities, then finish connecting.'}</li>
    </ol>
    <label className="account-file-choice">{approving ? 'Choose device request file' : 'Choose device approval file'}
      <input type="file" accept=".json,application/json" disabled={disabled || reading} onChange={event => { void choose(event.target.files?.[0]); event.target.value = ''; }} />
    </label>
    {reading ? <p role="status">Reading public device file…</p> : null}
    {error ? <p role="alert">{error}</p> : null}
    {packet ? <><p>{approving ? 'Device request ready to compare.' : 'Device approval ready to compare.'}</p><PublicPacketIdentity packet={packet} />
      <label className="account-confirm"><input type="checkbox" checked={confirmed} disabled={disabled || reading} onChange={event => setConfirmed(event.target.checked)} />I compared the complete account, library, request and device fingerprints on both devices using a trusted channel.</label>
      <button type="button" className="settings-account-primary" disabled={disabled || reading || !confirmed} onClick={() => void act({ action: approving ? 'approve_device' : 'accept_device_approval', packet: text })}>{approving ? 'Approve requested device' : 'Accept approval for this device'}</button>
    </> : null}
    {approving && status.custody?.deviceApproval === 'confirmed' ? <button type="button" disabled={disabled} onClick={() => void act({ action: 'device_approval_handed_off' })}>Confirm approval handed off</button> : null}
    <details className="settings-account-advanced"><summary>Advanced · paste public JSON instead</summary><label>Public device request or approval<textarea value={text} maxLength={8192} disabled={disabled || reading} onChange={event => { epoch.current++; accept(event.target.value, true); }} /></label>
      {!approving ? <button type="button" disabled={disabled || !status.retainedLibrary} onClick={() => void act({ action: 'show_pair_request' })}>Show public device request</button> : null}
    </details>
    <p>Device files contain public pairing information. Recovery files and recovery secrets are accepted only by the native recovery dialog.</p>
  </details>;
}
