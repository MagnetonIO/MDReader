import { useState } from 'react';
import type { SimpleAgentBackend } from '../../platform/agent-setup';
import { useAgentSetup } from './useAgentSetup';
import './agent-setup.css';

export function SimpleAgentSetup({ root, active, backend }: { root: string | null; active: boolean; backend?: SimpleAgentBackend }) {
  const setup = useAgentSetup(root, active, backend);
  const [copy, setCopy] = useState<{ command: string; message: string } | null>(null);
  const state = setup.state;
  const targetReady = setup.scope === 'user' || setup.project !== '';
  const sourceSelectionChanged = !!state && (setup.ids.length !== state.source_ids.length || setup.ids.some(id => !state.source_ids.includes(id)));
  return <section className="settings-section simple-agent-setup" aria-labelledby="simple-agent-title">
    <h3 id="simple-agent-title">Connect an agent</h3>
    <p>Let your agent search and read the sources you choose.</p>
    <div className="simple-agent-setup__target">
      <label>Agent<select value={setup.agent} disabled={setup.busy} onChange={event => setup.setAgent(event.target.value as 'claude' | 'codex')}>
        <option value="codex">Codex</option><option value="claude">Claude Code</option>
      </select></label>
      <label>Install for<select value={setup.scope} disabled={setup.busy} onChange={event => setup.setScope(event.target.value as 'project' | 'user')}>
        <option value="project">This project</option><option value="user">This user</option>
      </select></label>
      {setup.scope === 'project' ? <label>Project<select value={setup.project} disabled={setup.busy} onChange={event => setup.setProject(event.target.value)}>
        <option value="">Choose a project folder</option>
        {state?.projects.map(project => <option key={project.path} value={project.path}>{project.name} — {project.path}</option>)}
      </select></label> : null}
    </div>
    {setup.busy ? <p role="status">Checking agent setup…</p> : null}
    {setup.error ? <p role="alert">{setup.error}</p> : null}
    {setup.notice ? <p role="status">{setup.notice}</p> : null}
    {state ? <>
      <p role="status">{!state.supported ? 'Unavailable on this platform' : !state.installed ? state.enabled ? 'Configuration missing · MCP enabled' : 'Not installed' : state.enabled ? 'MCP enabled' : 'Installed · MCP off'}{state.client_version ? ` · ${state.client_version}` : ''}</p>
      {state.diagnostic ? <p role="status">{state.diagnostic}</p> : null}
      {state.installed ? <p>Configuration is installed. Connection has not been verified; check MDReader in your agent’s MCP tools.</p> : null}
      {state.command && targetReady ? <div className="simple-agent-setup__command">
        <label>Setup command<input readOnly value={state.command} onFocus={event => event.currentTarget.select()} /></label>
        <button type="button" className="settings-link-button" onClick={() => {
          const command = state.command!;
          void (async () => {
            try { await navigator.clipboard.writeText(command); setCopy({ command, message: 'Command copied. Run it in your terminal, then refresh status.' }); }
            catch { setCopy({ command, message: 'Copy unavailable. Select and copy the setup command.' }); }
          })();
        }}>Copy command</button>
        {copy?.command === state.command ? <p role="status">{copy.message}</p> : null}
      </div> : null}
      <fieldset disabled={setup.busy || !state.supported || !targetReady}>
        <legend>Sources this agent may read</legend>
        {state.sources.map(source => <label className="simple-agent-setup__source" key={source.id}>
          <input type="checkbox" checked={setup.ids.includes(source.id)} onChange={event => setup.selectSource(source.id, event.target.checked)} />{source.name}
        </label>)}
        {state.sources.length === 0 ? <p>Add a source in Library &amp; sources first.</p> : null}
      </fieldset>
      <div className="settings-account-actions">
        <button type="button" className="settings-link-button" disabled={setup.busy || !state.supported || !state.installed || !targetReady || setup.ids.length === 0 || state.enabled && !sourceSelectionChanged} onClick={() => void setup.enable()}>{state.enabled ? 'Save source selection' : 'Enable MCP'}</button>
        {state.enabled ? <button type="button" className="settings-link-button" disabled={setup.busy} onClick={() => void setup.disable()}>Disable MCP</button> : null}
      </div>
      {state.installed ? <details><summary>Remove integration</summary>
        <p>Remove MDReader’s configuration for this agent and install scope.</p>
        <button type="button" className="settings-link-button" disabled={setup.busy} onClick={() => void setup.uninstall()}>Uninstall integration</button>
      </details> : null}
    </> : null}
    <button type="button" className="settings-link-button" disabled={setup.busy || !active} onClick={() => void setup.refresh()}>Refresh status</button>
  </section>;
}
