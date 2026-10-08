import { useId } from 'react';
import type { NativeProviderKind, NativeProvidersSnapshot } from '../../platform/providers.js';

interface ProviderConnectionFormsProps {
  snapshot: NativeProvidersSnapshot;
  busy: boolean;
  canConnect: (kind: NativeProviderKind) => boolean;
  dropboxPath: string;
  pathValid: boolean;
  recursive: boolean;
  editing: boolean;
  onPath: (value: string) => void;
  onRecursive: (value: boolean) => void;
  onEditing: (value: boolean) => void;
  onConnect: (kind: NativeProviderKind) => void;
}

function Availability({ snapshot, kind, available }: {
  snapshot: NativeProvidersSnapshot; kind: NativeProviderKind; available: boolean;
}) {
  const diagnostic = snapshot.availability.find((value) => value.provider === kind)?.diagnostic;
  if (available) return null;
  return <div className="provider-connection__unavailable">
    <p>This connection is unavailable in this version of MDReader.</p>
    {diagnostic ? <details><summary>Connection details</summary><p>{diagnostic}</p></details> : null}
  </div>;
}

/** Connection options only; native custody remains responsible for authorization. */
export function ProviderConnectionForms(props: ProviderConnectionFormsProps) {
  const pathId = useId();
  const pathHintId = useId();
  const editingHintId = useId();
  const googleAvailable = props.canConnect('google_drive');
  const dropboxAvailable = props.canConnect('dropbox');
  return <div className="provider-connections" aria-label="New connections">
    <section className="provider-connection" aria-label="Google Drive connection">
      <header className="provider-connection__heading"><h3>Google Drive</h3><span>{googleAvailable ? 'Ready to connect' : 'Unavailable'}</span></header>
      <p>Select one or more Markdown files from your Drive. Whole-folder selection is not available yet.</p>
      <button type="button" disabled={props.busy || !googleAvailable} onClick={() => props.onConnect('google_drive')}>Choose Google Drive files</button>
      <Availability snapshot={props.snapshot} kind="google_drive" available={googleAvailable} />
      {googleAvailable ? <details className="provider-connection__details"><summary>How file access works</summary><p>MDReader can access only the files you select. Reconnecting asks you to select files again.</p></details> : null}
    </section>
    <section className="provider-connection" aria-label="Dropbox connection">
      <header className="provider-connection__heading"><h3>Dropbox</h3><span>{dropboxAvailable ? 'Ready to connect' : 'Unavailable'}</span></header>
      <p>Choose a Dropbox folder to read Markdown from.</p>
      {dropboxAvailable ? <fieldset className="provider-connection__options" disabled={props.busy}>
        <legend className="provider-connection__legend">Dropbox folder options</legend>
        <label className="provider-connection__field" htmlFor={pathId}>Dropbox folder</label>
        <input id={pathId} aria-label="Dropbox folder path" aria-describedby={pathHintId} aria-invalid={!props.pathValid} value={props.dropboxPath} onChange={(event) => props.onPath(event.target.value)} maxLength={4096} spellCheck={false} />
        <p id={pathHintId} className="provider-connection__hint">{props.pathValid ? 'Use a path such as /Notes. Choosing / includes your accessible Dropbox root.' : 'Enter a path starting with /, without . or .. folders.'}</p>
        <label className="provider-connection__check"><input type="checkbox" checked={props.recursive} onChange={(event) => props.onRecursive(event.target.checked)} /><span>Include subfolders</span></label>
        <label className="provider-connection__check"><input type="checkbox" checked={props.editing} aria-describedby={editingHintId} onChange={(event) => props.onEditing(event.target.checked)} /><span>Allow editing</span></label>
        <p id={editingHintId} className="provider-connection__hint">Read only by default. Editing asks Dropbox for write permission.</p>
        <details className="provider-connection__details"><summary>About reconnecting</summary><p>These folder and editing options also apply when you reconnect a Dropbox source. Reconnecting replaces its previous selection and permission. Unsaved edits stay available to keep or download.</p></details>
      </fieldset> : null}
      <button type="button" disabled={props.busy || !dropboxAvailable || !props.pathValid} onClick={() => props.onConnect('dropbox')}>Connect Dropbox folder</button>
      <Availability snapshot={props.snapshot} kind="dropbox" available={dropboxAvailable} />
    </section>
  </div>;
}
