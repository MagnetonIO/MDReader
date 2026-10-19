import { useEffect, useState } from 'react';
import type { AgentPlansSource } from '../../bindings';
import { readerBackend } from '../../platform/backend.js';
import { simpleAgentBackend } from '../../platform/agent-setup.js';
import { SettingsCommandError } from '../../platform/settings.js';
import { errorMessage } from './presentation.js';

export interface AgentPlansBackend {
  status(): Promise<AgentPlansSource>;
  enable(): Promise<AgentPlansSource>;
  disable(sourceId: string): Promise<unknown>;
  /** Reader sources the Claude Code user-scope grant may read or report on. */
  grantedSources(): Promise<string[]>;
}

const unwrap = async <T,>(response: Promise<{ status: 'ok'; data: T } | { status: 'error'; error: string }>) => {
  const result = await response;
  if (result.status === 'error') throw new Error(result.error);
  return result.data;
};

/** Native resolves the folder; disabling forgets only the read-only plans member. */
export const agentPlansBackend: AgentPlansBackend = {
  status: () => unwrap(readerBackend().agents.agentPlansSourceStatus()),
  enable: () => unwrap(readerBackend().agents.enableAgentPlansSource()),
  disable: async sourceId => {
    const result = await readerBackend().agents.disableAgentPlansSource(sourceId);
    if (result.status === 'error') throw new SettingsCommandError('library', result.error);
    return result.data;
  },
  grantedSources: async () => {
    const state = await simpleAgentBackend.state({ agent: 'claude', scope: 'user', project_root: null });
    return [...state.source_ids, ...(state.reporting_project_ids ?? [])];
  },
};

export function AgentPlansToggle({ backend = agentPlansBackend }: { backend?: AgentPlansBackend }) {
  const [plans, setPlans] = useState<AgentPlansSource | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [granted, setGranted] = useState<string[] | null>(null);
  useEffect(() => {
    let live = true;
    backend.status().then(next => { if (live) setPlans(next); }, cause => { if (live) setError(errorMessage(cause)); });
    return () => { live = false; };
  }, [backend]);
  const readerSourceId = plans?.source_id ? plans.reader_source_id : null;
  useEffect(() => {
    if (!readerSourceId) return;
    let live = true;
    // Unknown grant state shows no hint rather than a false warning.
    backend.grantedSources().then(ids => { if (live) setGranted(ids); }, () => { if (live) setGranted(null); });
    return () => { live = false; };
  }, [backend, readerSourceId]);
  const needsGrant = !!readerSourceId && granted !== null && !granted.includes(readerSourceId);
  const toggle = async (enabled: boolean) => {
    setBusy(true); setError(null);
    try {
      if (enabled) setPlans(await backend.enable());
      else if (plans?.source_id) { await backend.disable(plans.source_id); setPlans({ ...plans, source_id: null, reader_source_id: null }); }
    } catch (cause) { setError(errorMessage(cause)); } finally { setBusy(false); }
  };
  return <div className="agent-plans-toggle">
    <label className="agent-connection-card__service"><input type="checkbox" role="switch" checked={Boolean(plans?.source_id)}
      disabled={busy || !plans || !plans.path} onChange={event => { void toggle(event.target.checked); }} />Index Claude plans</label>
    <p className="settings-field__hint">Adds your Claude Code plans folder as a read-only source.</p>
    {needsGrant ? <p role="status">Grant agents access to Claude plans to link plans to sessions.</p> : null}
    {error ? <p role="alert">{error}</p> : null}
  </div>;
}
