import { useState } from 'react';
import { SimpleAgentSetup } from '../agent-setup/SimpleAgentSetup';
import { AgentCapturePanel } from '../../shell/AgentCapturePanel.js';

/** Folder discovery, explicit capture and retrieval are independent capabilities. */
export function AgentsSettings({ root, active = true, onManageSources }: { root: string | null; active?: boolean; onManageSources(): void }) {
  const [advanced, setAdvanced] = useState(false);
  return <>
    <h2 id="settings-agents-title">Agents &amp; integrations</h2>
    <SimpleAgentSetup root={root} active={active} />
    <section className="settings-section" aria-labelledby="settings-agent-files-title">
      <h3 id="settings-agent-files-title">Markdown files from agents</h3>
      <p>Markdown files an agent writes in your selected Library folders use normal indexing.</p>
      {root ? <p>Currently reading folder: <span>{root}</span></p> : null}
      <p>All chosen folders stay watched unless paused, even when you read another folder. Home or disk-wide indexing requires an explicit Advanced folder selection.</p>
      <button type="button" className="settings-link-button" onClick={onManageSources}>Manage indexed folders</button>
    </section>
    <section className="settings-section" aria-labelledby="settings-agent-retrieval-title">
      <h3 id="settings-agent-retrieval-title">Capture and retrieval</h3>
      <p>Capture separately saves new visible final outputs. MCP gives agents read-only retrieval of admitted documents; it does not import files.</p>
    </section>
    <details onToggle={event => setAdvanced(event.currentTarget.open)}>
      <summary>Advanced</summary>
      <p>Optional capture saves new visible final outputs separately. Capture is off for new projects until you enable it.</p>
      {advanced && active ? <AgentCapturePanel root={root} /> : null}
    </details>
  </>;
}
