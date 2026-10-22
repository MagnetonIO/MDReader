import type { AgentSetupSelection, SimpleAgentBackend } from '../../platform/agent-setup';
import { SessionTrackingSwitch } from '../agent-sessions/SessionTrackingSwitch';
import type { AgentActivityBackend } from '../agent-sessions/contracts';
import { PublicSkillCommand } from './PublicSkillCommand';
import { useAgentSetup } from './useAgentSetup';

function sameIds(left: readonly string[], right: readonly string[]) {
  return left.length === right.length && left.every(id => right.includes(id));
}
export function AgentConnectionCard({ agent, root, active, backend, trackingBackend }: {
  agent: AgentSetupSelection['agent']; root: string | null; active: boolean; backend: SimpleAgentBackend; trackingBackend?: AgentActivityBackend;
}) {
  const setup = useAgentSetup(root, active, backend, agent);
  const state = setup.state;
  const name = agent === 'codex' ? 'Codex' : 'Claude Code';
  const targetReady = setup.scope === 'user' || state?.projects.some(project => project.path === setup.project);
  const unavailable = !active || setup.busy || !state?.supported || !targetReady;
  const changed = !!state && (!sameIds(setup.ids, state.source_ids) || !sameIds(setup.editIds, state.edit_source_ids ?? []));
  const connected = !!state?.enabled && state.connected === true;
  const status = !state ? 'Checking connection…' : !state.supported ? 'Unavailable on this platform'
    : connected ? 'Connected' : state.enabled ? 'Waiting for agent' : state.installed ? 'Installed · Access off' : 'Not connected';
  return <article className="agent-connection-card" aria-label={`${name} connection`}>
    <div className="agent-connection-card__header"><h4>{name}</h4><span className={`agent-connection-card__status${connected ? ' agent-connection-card__status--connected' : ''}`} role="status">{setup.busy ? 'Working…' : status}</span></div>
    {state?.last_seen_ms ? <p className="agent-connection-card__seen">Last seen {new Date(state.last_seen_ms).toLocaleString()}</p> : null}
    {setup.error ? <p role="alert">{setup.error}</p> : null}
    {setup.notice ? <p role="status">{setup.notice}</p> : null}
    {state?.diagnostic ? <p role="status">{state.diagnostic}</p> : null}
    <SessionTrackingSwitch agent={agent} active={active} backend={trackingBackend} />
    {state ? <>
      <div className="agent-connection-card__actions">
        <label className="agent-connection-card__service"><input type="checkbox" role="switch" checked={state.enabled}
          disabled={unavailable || !state.installed && !state.client_version}
          onChange={event => { void (!event.target.checked ? setup.disable() : state.installed ? setup.enable() : setup.connect()); }} />Agent access (MCP)</label>
        {state.enabled && changed ? <button type="button" className="settings-link-button" disabled={unavailable} onClick={() => void setup.enable()}>Save access</button> : null}
      </div>
      {state.enabled && !connected ? <p>Open or restart {name} to finish connecting.</p> : null}
      {!state.client_version && state.supported ? <p>Install {name} first, then refresh status.</p> : null}
      <details className="agent-connection-card__sharing"><summary>Advanced: source access</summary><p>Let this agent read or change selected MDReader documents through MCP. Session tracking works without these permissions.</p>
        <div className="agent-connection-card__permissions" role="group" aria-label={`${name} document permissions`}>
          {state.sources.map(source => <div className="agent-connection-card__permission" key={source.id}><span>{source.name}</span><label><input type="checkbox" disabled={unavailable} checked={setup.ids.includes(source.id)} onChange={event => setup.selectSource(source.id, event.target.checked)} />Read</label><label><input type="checkbox" disabled={unavailable || source.editable === false} checked={setup.editIds.includes(source.id)} onChange={event => setup.selectEdit(source.id, event.target.checked)} />Create/edit</label></div>)}
          {!state.sources.length ? <p>Add a source in Library &amp; sources first.</p> : null}
        </div>
      </details>
    </> : null}
    <details className="simple-agent-setup__options"><summary>Advanced: installation options</summary>
      <fieldset className="simple-agent-setup__scope" disabled={setup.busy || !active}><legend>Install for</legend>{(['user', 'project'] as const).map(scope => <label key={scope}><input type="radio" name={`mdreader-install-scope-${agent}`} checked={setup.scope === scope} onChange={() => setup.setScope(scope)} />{scope === 'user' ? 'All projects' : 'One project'}</label>)}</fieldset>
      {setup.scope === 'project' ? <div className="simple-agent-setup__project"><label>Project<select value={setup.project} disabled={setup.busy || !active} onChange={event => setup.setProject(event.target.value)}><option value="">Choose a saved project</option>{state?.projects.map(project => <option key={project.path} value={project.path}>{project.name}</option>)}</select></label>{setup.canChooseFolder ? <button type="button" className="settings-link-button" disabled={!active || setup.busy} onClick={() => void setup.chooseFolder()}>Choose project folder</button> : null}</div> : null}
      <PublicSkillCommand command={state?.command ?? null} />
      {state?.client_version ? <p>Detected version: {state.client_version}</p> : null}
      {state?.installed ? <button type="button" className="settings-link-button" disabled={setup.busy || !active} onClick={() => void setup.uninstall()}>Uninstall integration</button> : null}
    </details>
    <button type="button" className="settings-link-button" disabled={setup.busy || !active} onClick={() => void setup.refresh()}>Refresh status</button>
  </article>;
}
