import { useState } from 'react';
import { SimpleAgentSetup } from '../agent-setup/SimpleAgentSetup';
import { AgentCapturePanel } from '../../shell/AgentCapturePanel.js';

/** Folder discovery, explicit capture and retrieval are independent capabilities. */
export function AgentsSettings({ root, active = true, onManageSources }: { root: string | null; active?: boolean; onManageSources(): void }) {
  const [advanced, setAdvanced] = useState(false);
  return <>
    <h2 id="settings-agents-title">Agents &amp; integrations</h2>
    <SimpleAgentSetup root={root} active={active} />
    <button type="button" className="settings-link-button" onClick={onManageSources}>Manage indexed folders</button>
    <details onToggle={event => setAdvanced(event.currentTarget.open)}>
      <summary>Advanced: optional response capture</summary>
      <p>Save new final responses separately. Off by default; enable it for a project here.</p>
      {advanced && active ? <AgentCapturePanel root={root} /> : null}
    </details>
  </>;
}
