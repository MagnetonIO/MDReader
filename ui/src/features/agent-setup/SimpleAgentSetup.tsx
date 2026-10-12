import { useMemo, useRef, useState } from 'react';
import { simpleAgentBackend, type SimpleAgentBackend } from '../../platform/agent-setup';
import { AgentConnectionCard } from './AgentConnectionCard';
import { agentSetupError } from './setup-error';
import './agent-setup.css';

export function SimpleAgentSetup({ root, active, backend = simpleAgentBackend }: { root: string | null; active: boolean; backend?: SimpleAgentBackend }) {
  const [refresh, setRefresh] = useState(0);
  const [picking, setPicking] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const identity = useMemo(() => ({}), [active, backend]);
  const current = useRef(identity); current.current = identity;
  async function chooseProject() {
    if (!active || picking || !backend.chooseProjectFolder) return;
    setPicking(true); setError(null);
    try {
      const selected = await backend.chooseProjectFolder();
      if (selected && current.current === identity) setRefresh(value => value + 1);
    } catch (cause) { if (current.current === identity) setError(agentSetupError(cause)); }
    finally { setPicking(false); }
  }
  return <section className="settings-section simple-agent-setup" aria-labelledby="simple-agent-title">
    <h3 id="simple-agent-title">Agent projects</h3>
    <p>Read the Markdown your agents create. Chosen project folders stay indexed automatically, including subfolders.</p>
    {backend.chooseProjectFolder ? <button type="button" className="settings-link-button" disabled={!active || picking} onClick={() => void chooseProject()}>{picking ? 'Choosing folder…' : 'Choose project folder'}</button> : null}
    {error ? <p role="alert">{error}</p> : null}
    <div className="agent-connection-cards">
      {(['codex', 'claude'] as const).map(agent => <AgentConnectionCard key={`${agent}:${refresh}`} agent={agent} root={root} active={active} backend={backend} refreshKey={refresh} />)}
    </div>
  </section>;
}
