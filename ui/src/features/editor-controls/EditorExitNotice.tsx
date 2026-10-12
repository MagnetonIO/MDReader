export function EditorExitNotice({ error, retry, saveCopy, copying = false }: {
  error: string | null; retry(): void; saveCopy(): void; copying?: boolean;
}) {
  if (!error) return null;
  return <div className="editor-exit-notice" role="alert">
    <p>{error}</p>
    <button type="button" onClick={retry}>Retry</button>
    <button type="button" disabled={copying} onClick={saveCopy}>{copying ? 'Saving copy…' : 'Save a copy…'}</button>
  </div>;
}
