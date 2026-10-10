import { useEffect, useRef, useState } from 'react';
import { agentIntegrationBackend, type AgentIntegrationBackend, type AgentIntegrationSelection, type AgentIntegrationReview, type AgentIntegrationsState } from '../platform/agents.js';

const choices: { value: AgentIntegrationSelection['kind']; label: string; hooks: boolean }[] = [
  { value: 'claude_hooks', label: 'Claude Code — capture new outputs', hooks: true },
  { value: 'codex_hooks', label: 'Codex local CLI — capture new outputs', hooks: true },
  { value: 'claude_mcp', label: 'Claude Code — search MDReader with MCP', hooks: false },
  { value: 'codex_mcp', label: 'Codex — search MDReader with MCP', hooks: false },
];
function CaptureSetup({ kind }: { kind: string }) {
  if (kind !== 'codex_hooks' && kind !== 'claude_hooks') return null;
  return <div className="agent-integrations__setup">
    <h4>{kind === 'codex_hooks' ? 'Finish setup in Codex' : 'Finish setup in Claude Code'}</h4>
    {kind === 'codex_hooks' ? <>
      <p>Start the local Codex CLI in this folder and complete its normal folder trust review. Open <code>/hooks</code>, review MDReader’s exact hook command, and approve that definition before relying on capture. A changed command needs approval again.</p>
      <p>Local command hooks do not run with cloud orchestration, even when tools run on your computer.</p>
    </> : <>
      <p>Open Claude Code in this folder, complete its normal project trust review, and use <code>/hooks</code> to check that MDReader’s project hooks are loaded. Follow any client approval prompts.</p>
      <p>Safe mode, disabled hooks or managed-only policy can prevent project hooks from running. Keep those controls in place; check with your administrator if policy blocks capture.</p>
    </>}
    <p>Installing configuration does not verify capture. After the client finishes a new visible response, check that it appears in the MDReader library and that the captured-occurrence count increases in Agent capture.</p>
  </div>;
}
export function AgentIntegrationPanel({ root, captureEnabled, backend = agentIntegrationBackend }: { root: string; captureEnabled: boolean; backend?: AgentIntegrationBackend }) {
  const [state, setState] = useState<AgentIntegrationsState | null>(null);
  const [kind, setKind] = useState<AgentIntegrationSelection['kind']>('claude_hooks');
  const [version, setVersion] = useState('');
  const [trusted, setTrusted] = useState(false);
  const [sources, setSources] = useState<string[]>([]);
  const [review, setReview] = useState<AgentIntegrationReview | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const serial = useRef(0);
  const choice = choices.find((c) => c.value === kind)!;
  useEffect(() => {
    const id = ++serial.current;
    setState(null); setReview(null); setError(null); setNotice(null); setSources([]); setTrusted(false); setVersion(''); setBusy(true);
    void backend.state(root).then((value) => { if (serial.current === id) setState(value); })
      .catch((cause: unknown) => { if (serial.current === id) setError(cause instanceof Error ? cause.message : 'Agent integrations are unavailable.'); })
      .finally(() => { if (serial.current === id) setBusy(false); });
    return () => { serial.current += 1; };
  }, [backend, root, captureEnabled]);
  async function act(operation: () => Promise<void>) {
    const id = ++serial.current; setBusy(true); setError(null); setNotice(null);
    try { await operation(); } catch (cause) {
      if (serial.current === id) setError(cause instanceof Error ? cause.message : 'The integration could not be changed.');
    } finally { if (serial.current === id) setBusy(false); }
  }
  const canPrepare = state && trusted && version.trim() && (choice.hooks ? captureEnabled : sources.length > 0);
  return <div className="agent-integrations">
    <h3>Connect an agent</h3>
    <p>Changes apply only to this folder. MDReader does not verify your client’s compatibility or bypass its project trust settings.</p>
    {error ? <p role="alert">{error}</p> : null}
    {notice ? <p role="status">{notice}</p> : null}
    <fieldset disabled={busy}>
      <label>Integration<select value={kind} onChange={(e) => { setKind(e.target.value as AgentIntegrationSelection['kind']); setReview(null); setTrusted(false); }}>
        {choices.map((c) => <option key={c.value} value={c.value}>{c.label}</option>)}
      </select></label>
      <CaptureSetup kind={kind} />
      <label>Installed agent version<input value={version} maxLength={128} placeholder="Check your agent’s --version output" onChange={(e) => { setVersion(e.target.value); setReview(null); }} /></label>
      {choice.hooks ? !captureEnabled ? <p>Enable capture for this folder before installing capture hooks.</p> : null : <fieldset>
        <legend>Sources this agent may read</legend>
        <p>MCP provides read-only document retrieval for the selected sources.</p>
        {state?.sources.map((s) => <label key={s.id}><input type="checkbox" checked={sources.includes(s.id)} onChange={(e) => { setSources((current) => e.target.checked ? [...current, s.id] : current.filter((id) => id !== s.id)); setReview(null); }} />{s.name} <small>{s.id}</small></label>)}
        {state?.sources.length === 0 ? <p>Add and index a folder first.</p> : null}
      </fieldset>}
      <label><input type="checkbox" checked={trusted} onChange={(e) => { setTrusted(e.target.checked); setReview(null); }} />I trust this folder and have confirmed this agent version supports the selected integration.</label>
      <button type="button" disabled={!canPrepare} onClick={() => void act(async () => {
        const id = serial.current;
        const value = await backend.prepare(root, { kind, client_version: version.trim(), compatibility_and_trust_confirmed: trusted, source_ids: choice.hooks ? [] : sources });
        const listed = await backend.state(root);
        if (serial.current === id) { setReview(value); setState(listed); }
      })}>Review configuration change</button>
    </fieldset>
    {review ? <div>
      <h4>Review: {review.configPath}</h4>
      <p>Client support has not been verified. Applying this change does not mean capture or MCP is active.</p>
      <details><summary>Current configuration</summary><pre>{review.beforeText ?? '(new file)'}</pre></details>
      <details open><summary>Proposed configuration</summary><pre>{review.afterText}</pre></details>
      <button type="button" disabled={busy} onClick={() => void act(async () => {
        const id = serial.current;
        await backend.apply(root, review.preparationId);
        const listed = await backend.state(root);
        if (serial.current === id) { setState(listed); setReview(null); setNotice(choice.hooks ? 'Configuration installed. Complete the client setup below, then verify a new output reaches MDReader.' : 'Configuration installed. Review and trust it in your agent, restart if required, then verify MCP retrieval.'); }
      })}>Apply reviewed change</button>
      <button type="button" disabled={busy} onClick={() => void act(async () => {
        const id = serial.current;
        await backend.uninstall(root, review.preparationId);
        const listed = await backend.state(root);
        if (serial.current === id) { setState(listed); setReview(null); }
      })}>Discard review</button>
    </div> : null}
    {state?.installations.filter((entry) => entry.state !== 'removed').map((entry) => <div key={entry.preparationId}>
      <p>{entry.kind.replaceAll('_', ' ')} — {entry.state}<br /><small>{entry.configPath}</small></p>
      {entry.state === 'applied' && entry.kind !== kind ? <CaptureSetup kind={entry.kind} /> : null}
      <button type="button" disabled={busy} onClick={() => void act(async () => {
        const id = serial.current;
        await backend.uninstall(root, entry.preparationId);
        const listed = await backend.state(root);
        if (serial.current === id) { setState(listed); setReview(null); setNotice(entry.state === 'prepared' ? 'Pending review discarded.' : 'MDReader’s owned configuration entries were removed.'); }
      })}>{entry.state === 'prepared' ? 'Discard pending review' : 'Remove integration'}</button>
    </div>)}
  </div>;
}
