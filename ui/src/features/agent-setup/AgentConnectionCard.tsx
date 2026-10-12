import type { AgentSetupSelection, SimpleAgentBackend } from '../../platform/agent-setup';
import { PublicSkillCommand } from './PublicSkillCommand';
import { useAgentSetup } from './useAgentSetup';

function sameIds(left: readonly string[], right: readonly string[]) {
  return left.length === right.length && left.every(id => right.includes(id));
}
export function AgentConnectionCard({ agent, root, active, backend, refreshKey }: {
  agent: AgentSetupSelection['agent']; root: string | null; active: boolean; backend: SimpleAgentBackend; refreshKey: number;
}) {
  const setup = useAgentSetup(root, active, backend, agent);
  const state = setup.state;
  const name = agent === 'codex' ? 'Codex' : 'Claude Code';
  const targetReady = setup.scope === 'user' || state?.projects.some(project => project.path === setup.project);
  const unavailable = !active || setup.busy || !state?.supported || !targetReady;
  const changed = !!state && (!sameIds(setup.ids, state.source_ids)
    || !sameIds(setup.projectIds, state.reporting_project_ids ?? []) || !sameIds(setup.editIds, state.edit_source_ids ?? []));
  const canEnable = setup.projectIds.length + setup.ids.length + setup.editIds.length > 0;
  const selectedProjectUnavailable = !!state && setup.projectIds.some(id => state.projects.some(project => project.id === id && (project.enabled === false || project.monitoring === false)));
  const connected = !!state?.enabled && state.connected === true;
  const status = !state ? 'Checking connection…' : !state.supported ? 'Unavailable on this platform'
    : connected ? 'Connected' : state.enabled ? 'Waiting for agent' : state.installed ? 'Installed · Service off' : 'Not connected';
  return <article className="agent-connection-card" aria-label={`${name} connection`} data-refresh={refreshKey}>
    <div className="agent-connection-card__header"><h4>{name}</h4><span className={`agent-connection-card__status${connected ? ' agent-connection-card__status--connected' : ''}`} role="status">{setup.busy ? 'Working…' : status}</span></div>
    {state?.last_seen_ms ? <p className="agent-connection-card__seen">Last seen {new Date(state.last_seen_ms).toLocaleString()}</p> : null}
    {setup.error ? <p role="alert">{setup.error}</p> : null}
    {setup.notice ? <p role="status">{setup.notice}</p> : null}
    {state?.diagnostic ? <p role="status">{state.diagnostic}</p> : null}
    {state ? <>
      <fieldset disabled={unavailable} className="agent-connection-card__projects"><legend>Projects to organize</legend>
        {state.projects.filter(project => project.id).map(project => <label key={project.id} className="simple-agent-setup__source"><input type="checkbox" disabled={!setup.projectIds.includes(project.id!) && (project.enabled === false || project.monitoring === false)} checked={setup.projectIds.includes(project.id!)} onChange={event => setup.selectProject(project.id!, event.target.checked)} />{project.name}{' '}{project.monitoring === false ? <span className="agent-connection-card__seen">Paused</span> : project.enabled === false ? <span className="agent-connection-card__seen">Unavailable</span> : null}</label>)}
        {!state.projects.some(project => project.id) ? <p>Choose a project folder to start.</p> : null}
      </fieldset>
      <details className="agent-connection-card__sharing"><summary>Optional source access</summary><p>Let this agent read or change selected MDReader documents. Project discovery works without these permissions.</p>
        <div className="agent-connection-card__permissions" role="group" aria-label={`${name} document permissions`}>
          {state.sources.map(source => <div className="agent-connection-card__permission" key={source.id}><span>{source.name}</span><label><input type="checkbox" disabled={unavailable} checked={setup.ids.includes(source.id)} onChange={event => setup.selectSource(source.id, event.target.checked)} />Read</label><label><input type="checkbox" disabled={unavailable || source.editable === false} checked={setup.editIds.includes(source.id)} onChange={event => setup.selectEdit(source.id, event.target.checked)} />Create/edit</label></div>)}
          {!state.sources.length ? <p>Add a source in Library &amp; sources first.</p> : null}
        </div>
      </details>
      <div className="agent-connection-card__actions">
        {!state.installed ? <button type="button" className="settings-link-button agent-connection-card__connect" disabled={unavailable || !state.client_version || !canEnable || selectedProjectUnavailable} onClick={() => void setup.connect()}>Connect {name}</button>
          : <label className="agent-connection-card__service"><input type="checkbox" role="switch" checked={state.enabled} disabled={unavailable || !state.enabled && (!canEnable || selectedProjectUnavailable)} onChange={event => { void (event.target.checked ? setup.enable() : setup.disable()); }} />Agent service</label>}
        {state.enabled && changed ? <button type="button" className="settings-link-button" disabled={unavailable || !canEnable || selectedProjectUnavailable} onClick={() => void setup.enable()}>Save access</button> : null}
      </div>
      {state.enabled && !connected ? <p>Open or restart {name} to finish connecting.</p> : null}
      {selectedProjectUnavailable ? <p>Resume the selected project in Library &amp; sources, or remove it from this connection.</p> : null}
      {!state.client_version && state.supported ? <p>Install {name} first, then refresh status.</p> : null}
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
