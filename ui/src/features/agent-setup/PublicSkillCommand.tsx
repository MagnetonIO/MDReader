import { useEffect, useRef, useState } from 'react';

/** Show only the public installer returned by native, never a private binary command. */
export function PublicSkillCommand({ command }: { command: string | null }) {
  const publicCommand = command && /^npx skills add MagnetonIO\/MDReader --skill mdreader --agent (codex|claude-code) --copy(?: --global)?$/.test(command) ? command : null;
  const latest = useRef(publicCommand); latest.current = publicCommand;
  useEffect(() => {
    latest.current = publicCommand;
    return () => { latest.current = null; };
  }, [publicCommand]);
  const [feedback, setFeedback] = useState<{ command: string; message: string } | null>(null);
  async function copy() {
    if (!publicCommand) return;
    const current = publicCommand;
    try {
      await navigator.clipboard.writeText(current);
      if (latest.current === current) setFeedback({ command: current, message: 'Command copied.' });
    } catch {
      if (latest.current === current) setFeedback({ command: current, message: 'Could not copy. Select and copy the command.' });
    }
  }
  return <section aria-label="Public agent skill">
    <p>The public skill adds MDReader instructions to your agent. It does not index folders or enable MCP.</p>
    {publicCommand ? <>
      <pre className="simple-agent-setup__command"><code>{publicCommand}</code></pre>
      {!publicCommand.endsWith(' --global') ? <p>Run this in the chosen project folder.</p> : null}
      <button type="button" className="settings-link-button" onClick={() => void copy()}>Copy command</button>
      {feedback?.command === publicCommand ? <p role="status">{feedback.message}</p> : null}
    </> : <p>The public skill command is unavailable. Install integration works directly in MDReader.</p>}
  </section>;
}
