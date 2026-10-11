import { useState } from 'react';
import type { SimpleAgentBackend } from '../../platform/agent-setup';
import { useAgentSetup } from './useAgentSetup';
import './agent-setup.css';

export function SimpleAgentSetup({ root, active, backend }: { root: string | null; active: boolean; backend?: SimpleAgentBackend }) {
  const setup = useAgentSetup(root, active, backend);
  const [copy, setCopy] = useState<{ command: string; message: string } | null>(null);
  const state = setup.state;
  const selectedProject = state?.projects.find(project => project.path === setup.project);
  const targetReady = setup.scope === 'user' || !!selectedProject;
  const sourceSelectionChanged = !!state && (setup.ids.length !== state.source_ids.length || setup.ids.some(id => !state.source_ids.includes(id)));
  const unavailable = !active || setup.busy || !state?.supported || !targetReady;
  return <section className="settings-section simple-agent-setup" aria-labelledby="simple-agent-title">
    <h3 id="simple-agent-title">Agent integration</h3>
    <fieldset className="simple-agent-setup__choices" disabled={setup.busy || !active}>
      <legend className="simple-agent-setup__legend">Choose an agent</legend>
      {(['codex', 'claude'] as const).map(agent => <label key={agent}>
        <input type="radio" name="mdreader-agent" checked={setup.agent === agent} onChange={() => setup.setAgent(agent)} />
        <span>{agent === 'codex' ? 'Codex' : 'Claude Code'}</span>
      </label>)}
    </fieldset>
    {setup.busy ? <p role="status">Working…</p> : null}
    {setup.error ? <p role="alert">{setup.error}</p> : null}
    {setup.notice ? <p role="status">{setup.notice}</p> : null}
    {state ? <>
      <p className="simple-agent-setup__status" role="status">{!state.supported ? 'Unavailable on this platform' : !state.installed ? state.enabled ? 'Configuration missing · MCP enabled' : 'Not installed' : state.enabled ? 'MCP enabled' : 'Installed · MCP off'}</p>
      {state.diagnostic ? <p role="status">{state.diagnostic}</p> : null}
      {!state.installed ? <>
        <p className="simple-agent-setup__target">{setup.scope === 'user' ? 'Install for all projects.' : selectedProject ? `Install for ${selectedProject.name}.` : 'Choose a project in Advanced installation options.'}</p>
        <button type="button" className="settings-link-button simple-agent-setup__install" disabled={unavailable || !state.client_version} onClick={() => void setup.install()}>Install integration</button>
        {!state.client_version && state.supported ? <p>Install {setup.agent === 'codex' ? 'Codex' : 'Claude Code'} first, then refresh status.</p> : null}
      </> : <p>Restart your agent to load the configuration. Choose sources, then enable MCP. Check MDReader in its MCP tools.</p>}
      {state.installed || state.enabled ? <>
        <fieldset className="simple-agent-setup__sources" disabled={unavailable}>
          <legend>Sources this agent may read</legend>
          {state.sources.map(source => <label className="simple-agent-setup__source" key={source.id}>
            <input type="checkbox" checked={setup.ids.includes(source.id)} onChange={event => setup.selectSource(source.id, event.target.checked)} />{source.name}
          </label>)}
          {state.sources.length === 0 ? <p>Add a source in Library &amp; sources first.</p> : null}
        </fieldset>
        <div className="settings-account-actions">
          <button type="button" className="settings-link-button" disabled={unavailable || !state.installed || setup.ids.length === 0 || state.enabled && !sourceSelectionChanged} onClick={() => void setup.enable()}>{state.enabled ? 'Save source selection' : 'Enable MCP'}</button>
          {state.enabled ? <button type="button" className="settings-link-button" disabled={setup.busy || !active} onClick={() => void setup.disable()}>Disable MCP</button> : null}
        </div>
      </> : null}
    </> : null}
    <details className="simple-agent-setup__options">
      <summary>Advanced: installation options</summary>
      <fieldset className="simple-agent-setup__scope" disabled={setup.busy || !active}>
        <legend>Install for</legend>
        {(['user', 'project'] as const).map(scope => <label key={scope}>
          <input type="radio" name="mdreader-install-scope" checked={setup.scope === scope} onChange={() => setup.setScope(scope)} />
          {scope === 'user' ? 'All projects' : 'One project'}
        </label>)}
      </fieldset>
      {setup.scope === 'project' ? <label className="simple-agent-setup__project">Project<select value={setup.project} disabled={setup.busy || !active} onChange={event => setup.setProject(event.target.value)}>
        <option value="">Choose a project folder</option>
        {state?.projects.map(project => <option key={project.path} value={project.path}>{project.name} — {project.path}</option>)}
      </select></label> : null}
      {state?.client_version ? <p>Detected version: {state.client_version}</p> : null}
      {state?.command && targetReady ? <div className="simple-agent-setup__command">
        <label>Terminal fallback<input readOnly value={state.command} onFocus={event => event.currentTarget.select()} /></label>
        <button type="button" className="settings-link-button" disabled={setup.busy || !active} onClick={() => {
          const command = state.command!;
          void (async () => {
            try { await navigator.clipboard.writeText(command); setCopy({ command, message: 'Command copied.' }); }
            catch { setCopy({ command, message: 'Copy unavailable. Select and copy the command.' }); }
          })();
        }}>Copy command</button>
        {copy?.command === state.command ? <p role="status">{copy.message}</p> : null}
      </div> : null}
      {state?.installed ? <button type="button" className="settings-link-button" disabled={setup.busy || !active} onClick={() => void setup.uninstall()}>Uninstall integration</button> : null}
    </details>
    <button type="button" className="settings-link-button" disabled={setup.busy || !active} onClick={() => void setup.refresh()}>Refresh status</button>
  </section>;
}
