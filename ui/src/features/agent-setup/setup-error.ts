/** Native rejections may be strings; only recognized public messages enter Settings. */
const messages: Record<string, string> = {
  'Agent configuration is too large for automatic setup.': 'Agent configuration is too large for automatic setup.',
  'Agent integration conflicts with existing settings. Existing files were kept.': 'Agent integration conflicts with existing settings. Existing files were kept.',
  'Document revision changed': 'Agent integration conflicts with existing settings. Existing files were kept.',
  'Choose an agent': 'Choose an agent.',
  'Choose an installation scope': 'Choose an installation scope.',
  'Choose a saved project folder': 'Choose a saved project folder.',
  'Choose a project folder': 'Choose a project folder.',
  'Selected folder unavailable': 'The selected folder is unavailable. Choose it again.',
  'Selected folder changed': 'The selected folder changed. Choose it again.',
  'Folder list unavailable': 'Folder list unavailable. Refresh status and try again.',
  'Agent inspection unavailable': 'Agent inspection unavailable. Refresh status and try again.',
  'Install the agent CLI, then try again': 'Install the agent CLI, then refresh status.',
  'Agent installation is available in desktop MDReader': 'Agent installation is available in desktop MDReader.',
  'Agent installation could not finish; existing files were retained': 'Agent installation could not finish. Existing files were kept.',
  'Setup status unavailable': 'Setup status unavailable. Refresh status and try again.',
};
export function agentSetupError(cause: unknown): string {
  const raw = typeof cause === 'string' ? cause : cause instanceof Error ? cause.message : '';
  const message = raw.startsWith('Invalid request: ') ? raw.slice('Invalid request: '.length) : raw;
  return messages[message] ?? 'Agent setup could not be completed. Existing files were kept. Refresh status and try again.';
}
