import { useEffect, useRef, useState } from 'react';
import { BackfillPanel } from '../agents/backfill/BackfillPanel.js';
import type { BackfillBackend } from '../agents/backfill/backend.js';
import { AgentIntegrationPanel } from './AgentIntegrationPanel.js';
import { agentIntegrationBackend, type AgentIntegrationBackend } from '../platform/agents.js';
import { agentCaptureBackend, type AgentCaptureBackend, type AgentCaptureState } from '../platform/agents.js';

const lines = (text: string) => text.split('\n').map((line) => line.trim()).filter(Boolean);

/** Project opt-in is independent of installing agent hooks. Opening settings is read-only. */
export function AgentCapturePanel({ root, backend = agentCaptureBackend, integrationBackend = agentIntegrationBackend, backfillBackend }: { root: string | null; backend?: AgentCaptureBackend; integrationBackend?: AgentIntegrationBackend | null; backfillBackend?: BackfillBackend }) {
  const [expanded, setExpanded] = useState(false);
  const [state, setState] = useState<AgentCaptureState | null>(null);
  const [enabled, setEnabled] = useState(false);
  const [subagents, setSubagents] = useState(false);
  const [paths, setPaths] = useState('');
  const [sessions, setSessions] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const serial = useRef(0);
  function accept(value: AgentCaptureState) {
    setState(value);
    setEnabled(value.enabled);
    setSubagents(value.capture_subagents);
    setPaths(value.excluded_paths.join('\n'));
    setSessions(value.excluded_sessions.join('\n'));
  }
  useEffect(() => {
    const id = ++serial.current;
    setState(null);
    setNotice(null);
    setError(null);
    setBusy(false);
    if (!root || !expanded) return;
    setBusy(true);
    void backend.state(root).then((value) => {
      if (serial.current === id && value.root === root) accept(value);
    }).catch((cause: unknown) => {
      if (serial.current === id) setError(cause instanceof Error ? cause.message : 'Agent capture settings are unavailable.');
    }).finally(() => {
      if (serial.current === id) setBusy(false);
    });
    return () => { serial.current += 1; };
  }, [backend, expanded, root]);
  async function save() {
    if (!root || !state || busy) return;
    const id = ++serial.current;
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      const value = await backend.update(root, {
        enabled, capture_subagents: subagents, excluded_paths: lines(paths), excluded_sessions: lines(sessions),
      });
      if (serial.current !== id || value.root !== root) return;
      accept(value);
      setNotice(value.enabled ? 'Capture is enabled for this folder. A supported agent integration must be installed to send new outputs.' : 'Capture is paused for this folder. Previously captured documents remain available.');
    } catch (cause) {
      if (serial.current === id) setError(cause instanceof Error ? cause.message : 'Agent capture settings could not be saved.');
    } finally {
      if (serial.current === id) setBusy(false);
    }
  }
  return <section className="settings-section" aria-labelledby="settings-agents">
    <div className="settings-section__intro"><div><h2 id="settings-agents">Agent capture</h2></div>
      <p>Keep visible plans and final responses from Claude Code and Codex as searchable Markdown.</p></div>
    <button type="button" aria-expanded={expanded} onClick={() => setExpanded((value) => !value)}>
      {expanded ? 'Hide agent capture' : 'Configure agent capture'}
    </button>
    {expanded ? !root ? <p>Open a local folder to configure capture for that project.</p> : <>
      <p>Selected folder: <span>{root}</span></p>
      <p>Only this folder is included. Existing histories are not imported. Hidden reasoning and raw tool output are excluded.</p>
      {busy && !state ? <p role="status">Loading capture settings…</p> : null}
      {error ? <p role="alert">{error}</p> : null}
      {notice ? <p role="status">{notice}</p> : null}
      {state ? <>
        {!state.supported ? <p>Agent capture is not supported on this platform yet.</p> : null}
        <fieldset disabled={busy || !state.supported}>
          <label><input type="checkbox" checked={enabled} onChange={(event) => setEnabled(event.target.checked)} /> Capture new visible outputs in this folder</label>
          <label><input type="checkbox" checked={subagents} onChange={(event) => setSubagents(event.target.checked)} /> Include subagent final responses</label>
          <label>Excluded folders (relative paths, one per line)<textarea value={paths} onChange={(event) => setPaths(event.target.value)} /></label>
          <label>Excluded session IDs (one per line)<textarea value={sessions} onChange={(event) => setSessions(event.target.value)} /></label>
          <button type="button" onClick={() => void save()}>Save capture settings</button>
        </fieldset>
        <p>Library capture queue: {state.pending_events} pending. {state.admitted_occurrences} captured occurrences.</p>
        {state.diagnostic ? <p role="status">{state.diagnostic}</p> : null}
        {state.supported && integrationBackend ? <AgentIntegrationPanel root={root} captureEnabled={state.enabled} backend={integrationBackend} /> : null}
      </> : null}
      {state?.supported !== false ? <BackfillPanel root={root} captureEnabled={state?.enabled ?? false} backend={backfillBackend} /> : null}
    </> : null}
  </section>;
}
