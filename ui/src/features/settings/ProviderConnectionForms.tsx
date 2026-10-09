import { useId } from 'react';
import type { NativeProviderKind, NativeProvidersSnapshot } from '../../platform/providers.js';
import { ProviderIcon } from '../providers/ProviderIcon.js';

interface ProviderConnectionFormsProps {
  snapshot: NativeProvidersSnapshot;
  busy: boolean;
  canConnect: (kind: NativeProviderKind) => boolean;
  dropboxPath: string;
  pathValid: boolean;
  recursive: boolean;
  editing: boolean;
  driveRecursive: boolean;
  driveEditing: boolean;
  allowEditing?: boolean;
  onPath: (value: string) => void;
  onRecursive: (value: boolean) => void;
  onEditing: (value: boolean) => void;
  onDriveRecursive: (value: boolean) => void;
  onDriveEditing: (value: boolean) => void;
  onConnect: (kind: NativeProviderKind) => void;
  onConnectGoogleFiles: () => void;
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
      <header className="provider-connection__heading"><h3 className="provider-connection__title"><ProviderIcon provider="google_drive" />Google Drive</h3><span>{googleAvailable ? 'Ready to connect' : 'Unavailable'}</span></header>
      <p>Connect your Drive to browse Markdown files. MDReader lists .md and .markdown documents; it downloads a document only when you open it.</p>
      {googleAvailable ? <fieldset className="provider-connection__options" disabled={props.busy}>
        <legend className="provider-connection__legend">New Google Drive connection options</legend>
        <label className="provider-connection__check"><input type="checkbox" checked={props.driveRecursive} onChange={event => props.onDriveRecursive(event.target.checked)} /><span>Include subfolders</span></label>
        {props.allowEditing !== false ? <label className="provider-connection__check"><input type="checkbox" checked={props.driveEditing} onChange={event => props.onDriveEditing(event.target.checked)} /><span>Allow editing</span></label> : null}
        <p className="provider-connection__hint">{props.driveRecursive ? 'Markdown in My Drive and its subfolders.' : 'Markdown directly in My Drive.'} Folder connections use these options too. Existing connections keep their permissions.</p>
      </fieldset> : null}
      <button type="button" disabled={props.busy || !googleAvailable} onClick={() => props.onConnect('google_drive')}>Connect Google Drive</button>
      <Availability snapshot={props.snapshot} kind="google_drive" available={googleAvailable} />
      <p className="provider-connection__hint">{props.allowEditing !== false && props.driveEditing
        ? 'Google grants Drive read and write access. MDReader limits edits to Markdown in your selected folders and saves only against a verified unchanged revision.'
        : 'Google grants read access to your Drive. MDReader lists selected Markdown and reads the documents you choose. Read only by default.'}</p>
      {googleAvailable ? <details className="provider-connection__details"><summary>Connect specific files instead</summary>
        <p>For access limited to selected files, use Google's file picker. Existing selected-file connections keep their original permission.</p>
        <button type="button" disabled={props.busy} onClick={props.onConnectGoogleFiles}>Choose Google Drive files</button>
      </details> : null}
    </section>
    <section className="provider-connection" aria-label="Dropbox connection">
      <header className="provider-connection__heading"><h3 className="provider-connection__title"><ProviderIcon provider="dropbox" />Dropbox</h3><span>{dropboxAvailable ? 'Ready to connect' : 'Unavailable'}</span></header>
      <p>Choose a Dropbox folder to read Markdown from.</p>
      {dropboxAvailable ? <fieldset className="provider-connection__options" disabled={props.busy}>
        <legend className="provider-connection__legend">Dropbox folder options</legend>
        <label className="provider-connection__field" htmlFor={pathId}>Dropbox folder</label>
        <input id={pathId} aria-label="Dropbox folder path" aria-describedby={pathHintId} aria-invalid={!props.pathValid} value={props.dropboxPath} onChange={(event) => props.onPath(event.target.value)} maxLength={4096} spellCheck={false} />
        <p id={pathHintId} className="provider-connection__hint">{props.pathValid ? 'Use a path such as /Notes. Choosing / includes your accessible Dropbox root.' : 'Enter a path starting with /, without . or .. folders.'}</p>
        <label className="provider-connection__check"><input type="checkbox" checked={props.recursive} onChange={(event) => props.onRecursive(event.target.checked)} /><span>Include subfolders</span></label>
        {props.allowEditing !== false ? <><label className="provider-connection__check"><input type="checkbox" checked={props.editing} aria-describedby={editingHintId} onChange={(event) => props.onEditing(event.target.checked)} /><span>Allow editing</span></label>
        <p id={editingHintId} className="provider-connection__hint">Read only by default. Editing asks Dropbox for write permission.</p></> : <p className="provider-connection__hint">Read only. Connecting does not grant permission to edit provider files.</p>}
        <details className="provider-connection__details"><summary>About reconnecting</summary><p>These folder and editing options also apply when you reconnect a Dropbox source. Reconnecting replaces its previous selection and permission. Unsaved edits stay available to keep or download.</p></details>
      </fieldset> : null}
      <button type="button" disabled={props.busy || !dropboxAvailable || !props.pathValid} onClick={() => props.onConnect('dropbox')}>Connect Dropbox folder</button>
      <Availability snapshot={props.snapshot} kind="dropbox" available={dropboxAvailable} />
    </section>
  </div>;
}
