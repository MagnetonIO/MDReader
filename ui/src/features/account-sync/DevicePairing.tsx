import { useEffect, useRef, useState } from 'react';
import type { AccountAction, AccountStatus } from './backend';
import { parsePublicPacket, type PublicPairPacket } from './public-packet';
import { PublicPacketExport } from './PublicPacketExport';
import { currentPairPacket } from './pairing-view';
import { PublicPacketIdentity } from './PublicPacketIdentity';
import { usePairPacket } from './usePairPacket';

export function DevicePairing({ status, disabled, act, onRecovery, packetToShare }: {
  status: AccountStatus; disabled: boolean; act(action: AccountAction): Promise<void>; onRecovery?(): void;
  packetToShare?: ReturnType<typeof currentPairPacket>;
}) {
  const [text, setText] = useState('');
  const [packet, setPacket] = useState<PublicPairPacket | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [confirmed, setConfirmed] = useState(false);
  const [reading, setReading] = useState(false);
  const epoch = useRef(0);
  const approving = status.connected;
  const retained = usePairPacket(status, approving ? 'approval' : 'request');
  const exported = packetToShare === undefined ? retained : packetToShare;
  const approvalRequired = !approving && status.custody?.pairRenewal === 'approval_required' && !status.custody.materialStaged;
  const renewalPending = !approving && (status.custody?.pairRenewal === 'prepared' || status.custody?.pairRenewal === 'uncertain');
  const requestReady = !approving && Boolean(exported) && !status.custody?.enrollmentExpired && !status.custody?.materialStaged && !renewalPending && !approvalRequired;
  const receiving = !status.custody?.materialStaged && (approvalRequired || !status.custody?.enrollmentExpired && !renewalPending);
  const expected = approving ? 'mdreader-pair-request' : 'mdreader-pair-approval';
  const owner = `${status.activeLibraryId}:${status.selectedScope?.libraryId}:${status.selectedScope?.accountDigest}:${approving}:${exported?.packet.pairId}:${status.custody?.pairRequestId}:${status.custody?.pairRenewal}:${receiving}`;
  useEffect(() => { epoch.current++; setText(''); setPacket(null); setError(null); setConfirmed(false); setReading(false); return () => { epoch.current++; }; }, [owner]);
  function accept(value: string, pasted = false) {
    setText(pasted ? value : ''); setConfirmed(false); setPacket(null); setError(null);
    try {
      const parsed = parsePublicPacket(value);
      if (parsed.kind !== expected) throw new Error(approving ? 'Choose the request file from the new device.' : 'Choose the approval file from your connected device.');
      if (status.activeLibraryId && parsed.libraryId !== status.activeLibraryId || status.selectedScope && (parsed.account !== status.selectedScope.accountDigest || parsed.libraryId !== status.selectedScope.libraryId)) throw new Error('This file belongs to another account or library. Your current connection is kept.');
      if (!approving && exported && parsed.pairId !== exported.packet.pairId) throw new Error('This approval belongs to another device request. Choose the approval for the request shown here.');
      if (!approving && status.custody?.pairRequestId && parsed.pairId !== status.custody.pairRequestId) throw new Error('This approval belongs to another device request. Choose the approval for this device’s retained request.');
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
  const importControls = <>
    <label className="account-file-choice">{approving ? 'Choose device request file' : 'Choose device approval file'}
      <input type="file" accept=".json,application/json" disabled={disabled || reading} onChange={event => { void choose(event.target.files?.[0]); event.target.value = ''; }} />
    </label>
    {reading ? <p role="status">Reading public device file…</p> : null}
    {error ? <p role="alert">{error}</p> : null}
    {packet ? <><p>{approving ? 'Device request ready to compare.' : 'Device approval ready to compare.'}</p><PublicPacketIdentity packet={packet} />
      <label className="account-confirm"><input type="checkbox" checked={confirmed} disabled={disabled || reading} onChange={event => setConfirmed(event.target.checked)} />I compared the complete account, library, request and device fingerprints on both devices using a trusted channel.</label>
      <button type="button" className="settings-account-primary" disabled={disabled || reading || !confirmed || !approving && !receiving} onClick={() => void act({ action: approving ? 'approve_device' : 'accept_device_approval', packet: text })}>{approving ? 'Approve requested device' : 'Accept approval for this device'}</button>
    </> : null}
  </>;
  const content = <>
    {approving ? <ol className="account-guided-steps">
      <li>On the new device, save its device request file and choose it here.</li>
      <li>Compare the complete identities on both devices before approving.</li>
      <li>Return the approval file through a trusted channel.</li>
    </ol> : requestReady ? <ol className="account-guided-steps account-pairing-steps">
      <li><strong>Send the request to your connected device</strong><p>Open Account &amp; sync → Add another device there, choose this request and save the approval.</p>
        <PublicPacketExport key={status.latestJob?.id} text={exported?.text ?? ''} kind="request" compact />
      </li>
      <li><strong>Return here with the approval</strong><p>Choose the approval file below, compare the full identities and accept it.</p></li>
    </ol> : approvalRequired ? <><p>Your connected device already approved the original request. Get that original approval file through a trusted channel, then compare the complete identities before accepting it here.</p><p>Your retained library and device keys are kept. MDReader does not retrieve or trust an approval automatically.</p></>
    : <p>{status.custody?.materialStaged ? 'Approval is saved. Check connection to finish the retained setup.'
      : renewalPending ? 'Retry the saved restart above before sharing a request. No replacement request is ready to send yet.'
      : status.custody?.enrollmentExpired ? 'Restart the connection request above. Once the new request is ready, send it to your connected device through a trusted channel and return with its approval. Your library and device keys are kept.'
      : 'Continue the connection above to show your saved device request. Then get approval from a connected device.'}</p>}
    {!approving && status.custody?.enrollmentExpired && !status.custody.materialStaged && !approvalRequired && !renewalPending && onRecovery ? <button type="button" onClick={onRecovery}>Other recovery options</button> : null}
    {approving && exported ? <PublicPacketExport key={status.latestJob?.id} text={exported.text} kind="approval" compact /> : null}
    {approving || receiving ? (requestReady || approving || approvalRequired ? importControls
      : <details className="settings-account-advanced"><summary>I already have an approval file</summary>{importControls}</details>) : null}
    {approving && status.custody?.deviceApproval === 'confirmed' ? <button type="button" disabled={disabled} onClick={() => void act({ action: 'device_approval_handed_off' })}>Confirm approval handed off</button> : null}
    {approving || receiving ? <details className="settings-account-advanced"><summary>Advanced · paste public JSON instead</summary><label>Public device request or approval<textarea value={text} maxLength={8192} disabled={disabled || reading} onChange={event => { epoch.current++; accept(event.target.value, true); }} /></label>
      {!approving && !approvalRequired ? <button type="button" disabled={disabled || !status.retainedLibrary || !receiving} onClick={() => void act({ action: 'show_pair_request' })}>Show public device request</button> : null}
    </details> : null}
    {approving || receiving ? <p>Device files contain public pairing information. Recovery files and recovery secrets are accepted only by the native recovery dialog.</p> : null}
  </>;
  return approving ? <details className="settings-account-advanced" open={exported ? true : undefined}>
    <summary>Add another device</summary>{content}
  </details> : <section className="account-pairing-flow" aria-label="Connect this device"><h4>{approvalRequired ? 'Finish the original request' : status.custody?.enrollmentExpired && !status.custody.materialStaged ? 'Restart this connection' : 'Approve this device'}</h4>{content}</section>;

}
