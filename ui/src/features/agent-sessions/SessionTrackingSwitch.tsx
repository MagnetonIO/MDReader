import { useEffect, useState } from 'react';
import { agentActivityBackend } from './backend';
import { agentName, type AgentActivityBackend, type AgentKind, type AgentTrackingStatus } from './contracts';

/** Opt-in reading of one client's local session logs; turning it off keeps what was recorded until removed. */
export function SessionTrackingSwitch({ agent, active, backend = agentActivityBackend }: { agent: AgentKind; active: boolean; backend?: AgentActivityBackend }) {
  const [status, setStatus] = useState<AgentTrackingStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [offerRemove, setOfferRemove] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [unknown, setUnknown] = useState(false);
  const name = agentName(agent);
  useEffect(() => {
    if (!active) return;
    let live = true;
    backend.trackingState().then(state => { if (live) setStatus(state[agent]); }, () => { if (live) setUnknown(true); });
    return () => { live = false; };
  }, [active, agent, backend]);
  async function run(task: () => Promise<void>, failure: string) {
    setBusy(true); setError(null); setNotice(null);
    try { await task(); } catch { setError(failure); } finally { setBusy(false); }
  }
  const toggle = (enabled: boolean) => run(async () => {
    const next = await backend.setTracking(agent, enabled);
    setStatus(next[agent]); setOfferRemove(!enabled); setConfirming(false);
  }, `${name} session tracking could not be changed.`);
  const forget = () => run(async () => {
    await backend.forget(agent);
    setOfferRemove(false); setConfirming(false); setNotice(`Tracked ${name} sessions were removed. Your files were not changed.`);
  }, `Tracked ${name} sessions could not be removed.`);
  return <div className="agent-connection-card__tracking">
    <label className="agent-connection-card__service"><input type="checkbox" role="switch" checked={!!status?.enabled}
      disabled={!active || busy || !status || !status.available && !status.enabled} onChange={event => void toggle(event.target.checked)} />Track sessions</label>
    <p className="settings-field__hint">{status && !status.available && !status.enabled
      ? `No ${name} session logs were found on this device.`
      : `Lists the Markdown files ${name} creates or edits, grouped by session. Reads titles and file paths from its local logs, never message text.`}</p>
    {offerRemove && !confirming ? <button type="button" className="settings-link-button" disabled={busy} onClick={() => setConfirming(true)}>Remove tracked sessions</button> : null}
    {confirming ? <div role="group" aria-label={`Remove tracked ${name} sessions`} className="agent-connection-card__actions">
      <span>Remove {name} sessions from MDReader? Files stay where they are.</span>
      <button type="button" className="settings-link-button" disabled={busy} onClick={() => void forget()}>Remove</button>
      <button type="button" className="settings-link-button" disabled={busy} onClick={() => setConfirming(false)}>Cancel</button>
    </div> : null}
    {notice ? <p role="status">{notice}</p> : null}
    {unknown && !status ? <p role="status">Session tracking status is unavailable.</p> : null}
    {error ? <p role="alert">{error}</p> : null}
  </div>;
}
