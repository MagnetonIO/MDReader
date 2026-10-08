import { useEffect, useRef, useState } from 'react';
import { nativeBackfillBackend, selectedEvent, type AgentBackfillAdapter, type AgentBackfillReply, type AgentBackfillReview, type BackfillBackend } from './backend';
type Snapshot = Extract<AgentBackfillReply, { operation: 'state' }>;
const message = (cause: unknown) => cause instanceof Error ? cause.message : 'Selected output backfill is unavailable.';
/** Selected sanitized outputs only. Opening this panel never scans agent files. */
export function BackfillPanel({ root, captureEnabled, backend = nativeBackfillBackend }: { root: string; captureEnabled: boolean; backend?: BackfillBackend }) {
  const [expanded, setExpanded] = useState(false);
  const [adapter, setAdapter] = useState<AgentBackfillAdapter>('claude_code');
  const [snapshot, setSnapshot] = useState<{ root: string; value: Snapshot } | null>(null);
  const [review, setReview] = useState<{ root: string; adapter: AgentBackfillAdapter; value: AgentBackfillReview } | null>(null);
  const [consent, setConsent] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const epoch = useRef(0);
  const running = useRef(false);
  const visible = review?.root === root && review.adapter === adapter ? review.value : null;
  const currentState = snapshot?.root === root ? snapshot.value : null;
  useEffect(() => {
    const id = ++epoch.current; running.current = false;
    setSnapshot(null); setReview(null); setConsent(false); setError(null); setNotice(null); setBusy(false);
    if (!expanded) return;
    setBusy(true); running.current = true;
    void backend.request(root, { operation: 'state' }).then((reply) => {
      if (epoch.current !== id) return;
      if (reply.operation !== 'state') throw new Error('Backfill selection metadata could not be verified.');
      setSnapshot({ root, value: reply });
    }).catch((cause: unknown) => { if (epoch.current === id) setError(message(cause)); })
      .finally(() => { if (epoch.current === id) { setBusy(false); running.current = false; } });
    return () => { epoch.current++; };
  }, [backend, root, captureEnabled, expanded]);
  async function refresh(id: number) {
    const reply = await backend.request(root, { operation: 'state' });
    if (epoch.current === id && reply.operation === 'state') setSnapshot({ root, value: reply });
  }
  async function operation(run: (id: number) => Promise<void>) {
    if (running.current) return;
    running.current = true; const id = ++epoch.current;
    setBusy(true); setError(null); setNotice(null);
    try { await run(id); }
    catch (cause) { if (epoch.current === id) setError(message(cause)); }
    finally { if (epoch.current === id) { running.current = false; setBusy(false); } }
  }
  function choose(file: File) {
    setReview(null); setConsent(false);
    void operation(async (id) => {
      const event = await selectedEvent(file);
      if (epoch.current !== id) return;
      const reply = await backend.request(root, { operation: 'review', adapter, event });
      if (epoch.current !== id) return;
      if (reply.operation !== 'reviewed') throw new Error('The selected visible output could not be reviewed.');
      // Only canonical parsed visible fields survive review. Raw input is never
      // stored, rendered, or reused for confirmation.
      setReview({ root, adapter, value: reply.value });
    });
  }
  function enqueue() {
    if (!visible || !consent || !captureEnabled || !currentState?.enabled) return;
    void operation(async (id) => {
      try {
        const reply = await backend.request(root, { operation: 'enqueue', adapter, event: visible.canonical_event,
          review_digest: visible.review_digest, session_id: visible.session_id });
        if (epoch.current !== id) return;
        if (reply.operation !== 'enqueued') throw new Error('Queue publication was not confirmed. Retry this same reviewed output to reconcile.');
        setConsent(false);
        setNotice(reply.outcome.status === 'queued' ? 'Output durably queued. Projection and indexing may still be pending.'
          : reply.outcome.status === 'duplicate' ? 'This same output occurrence is already queued or captured. No duplicate authorship was added.'
            : `Output was not queued: ${reply.outcome.reason}.`);
      } finally {
        if (epoch.current === id) await refresh(id);
      }
    });
  }
  function deselect(session: string) {
    void operation(async (id) => {
      const reply = await backend.request(root, { operation: 'deselect', session_id: session });
      if (epoch.current !== id) return;
      if (reply.operation !== 'deselected' || reply.session_id !== session) throw new Error('Session deselection was not confirmed. Reload this panel to reconcile.');
      setConsent(false); setReview(null);
      setNotice('This session is no longer selected for backfill. Pending events remain retained for explicit recovery; live capture settings are unchanged.');
      await refresh(id);
    });
  }
  return <section className="agent-capture-panel" aria-label="Selected past outputs">
    <button type="button" aria-expanded={expanded} onClick={() => setExpanded((value) => !value)}>
      {expanded ? 'Hide selected past outputs' : 'Import selected past output'}
    </button>
    {expanded ? <>
      <p>Choose one exported Claude Code or Codex final-output JSON event. No agent directory, transcript, plan file or tool log is scanned.
        The event supplies its session, optional turn and working folder; missing provenance is not reconstructed.</p>
      {!captureEnabled || currentState && !currentState.enabled ? <p>Enable capture for this folder before submitting a past output.</p> : null}
      <fieldset disabled={busy}>
        <label>Agent event format<select value={adapter} onChange={(event) => {
          setAdapter(event.currentTarget.value as AgentBackfillAdapter); setReview(null); setConsent(false); setError(null); setNotice(null);
        }}>
          <option value="claude_code">Claude Code Stop event</option>
          <option value="codex_hook">Codex Stop event</option>
          <option value="codex_notify">Codex agent-turn-complete notification</option>
        </select></label>
        <label>Choose one final-output JSON event<input type="file" accept="application/json,.json" disabled={!captureEnabled || !currentState?.enabled}
          onChange={(event) => { const file = event.currentTarget.files?.[0]; event.currentTarget.value = ''; if (file) choose(file); }} /></label>
        {visible ? <>
          <dl><dt>Supplied session</dt><dd><code>{visible.session_id}</code></dd>
            <dt>Supplied turn</dt><dd>{visible.turn_id ? `${visible.turn_id_kind}: ${visible.turn_id}` : 'Unavailable — no turn identifier will be invented'}</dd>
            <dt>Subagent</dt><dd>{visible.subagent_id ?? 'Not supplied'}</dd>
            <dt>Working folder</dt><dd>{visible.cwd}</dd></dl>
          <p>Original event time and repository revision are unavailable. Capture time records this import, and the chosen format identifies the supplied agent; it does not verify authorship.</p>
          <details><summary>Review exact visible output</summary><pre>{visible.body}</pre></details>
          <label><input type="checkbox" checked={consent} onChange={(event) => setConsent(event.currentTarget.checked)} />
            I reviewed this visible output and its supplied provenance, and select this session for backfill.</label>
          <button type="button" disabled={!consent || !captureEnabled || !currentState?.enabled} onClick={enqueue}>Select this session and queue this output</button>
          <p>A lost reply may still have queued the output. Explicitly retry this same reviewed event to reconcile safely.</p>
        </> : null}
        {currentState?.selected_sessions.length ? <div><h3>Sessions selected for backfill</h3>
          {currentState.selected_sessions.map((session, index) => <p key={`${index}:${session}`}><code>{session}</code>{' '}
            <button type="button" onClick={() => deselect(session)}>Stop backfill for session {session}</button></p>)}
        </div> : null}
      </fieldset>
      {busy ? <p role="status">Processing selected backfill request…</p> : null}
      {notice ? <p role="status">{notice}</p> : null}
      {error ? <p role="alert">{error}</p> : null}
    </> : null}
  </section>;
}
