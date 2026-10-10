import type { WatcherStatus } from '../../stores/sources.js';
import { AgentCapturePanel } from '../../shell/AgentCapturePanel.js';

/** Folder discovery, explicit capture and retrieval are independent capabilities. */
export function AgentsSettings({ root, watcher, onManageSources }: { root: string | null; watcher: WatcherStatus; onManageSources(): void }) {
  return <>
    <h2 id="settings-agents-title">Agents &amp; integrations</h2>
    <section className="settings-section" aria-labelledby="settings-agent-files-title">
      <h3 id="settings-agent-files-title">Markdown files from agents</h3>
      <p>Markdown files an agent writes in your active folder and its subfolders use normal indexing.</p>
      {root ? <p>{watcher === 'watching' ? 'Active watched folder' : 'Active folder'}: <span>{root}</span></p> : <p>No local folder is active. Choose a folder in Library &amp; sources.</p>}
      {root && watcher !== 'watching' ? <p>Folder watching is {watcher === 'error' ? 'unavailable' : 'paused'}.</p> : null}
      <p>Inactive saved folders are not watched. MDReader does not scan your home folder.</p>
      <button type="button" className="settings-link-button" onClick={onManageSources}>Manage indexed folders</button>
    </section>
    <section className="settings-section" aria-labelledby="settings-agent-retrieval-title">
      <h3 id="settings-agent-retrieval-title">Capture and retrieval</h3>
      <p>Capture separately saves new visible final outputs. MCP gives agents read-only retrieval of admitted documents; it does not import files.</p>
    </section>
    <AgentCapturePanel root={root} />
  </>;
}
