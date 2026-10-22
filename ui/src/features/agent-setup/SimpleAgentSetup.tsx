import { simpleAgentBackend, type SimpleAgentBackend } from '../../platform/agent-setup';
import type { AgentActivityBackend } from '../agent-sessions/contracts';
import { AgentConnectionCard } from './AgentConnectionCard';
import './agent-setup.css';

export function SimpleAgentSetup({ root, active, backend = simpleAgentBackend, trackingBackend }: {
  root: string | null; active: boolean; backend?: SimpleAgentBackend; trackingBackend?: AgentActivityBackend;
}) {
  return <section className="settings-section simple-agent-setup" aria-labelledby="simple-agent-title">
    <h3 id="simple-agent-title">Agent sessions</h3>
    <p>See the Markdown files your agents create or edit, grouped by session in the sidebar. Tracking and agent access are separate choices for each client.</p>
    <div className="agent-connection-cards">
      {(['codex', 'claude'] as const).map(agent => <AgentConnectionCard key={agent} agent={agent} root={root} active={active} backend={backend} trackingBackend={trackingBackend} />)}
    </div>
  </section>;
}
