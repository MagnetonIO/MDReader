import { useState } from 'react';
import './folder-indexing.css';

/** Broad roots require explicit confirmation; the native picker grants only the chosen root. */
export function BroadFolderSelection({ busy, onChoose }: { busy: boolean; onChoose(scope: 'home' | 'disk'): void }) {
  const [scope, setScope] = useState<'home' | 'disk'>('home');
  const [confirmed, setConfirmed] = useState(false);
  return <details className="settings-broad-folders">
    <summary>Advanced folder selection</summary>
    <p>Home and disk-wide selections can include many files. Choose the root explicitly; private MDReader data remains excluded.</p>
    <fieldset disabled={busy}>
      <label>Scope<select value={scope} onChange={event => { setScope(event.target.value as 'home' | 'disk'); setConfirmed(false); }}>
        <option value="home">Home folder</option><option value="disk">Disk</option>
      </select></label>
      <label><input type="checkbox" checked={confirmed} onChange={event => setConfirmed(event.target.checked)} />I want to index Markdown within this broad selection.</label>
      <button type="button" className="settings-link-button" disabled={!confirmed || busy} onClick={() => onChoose(scope)}>{busy ? 'Choosing…' : 'Choose folder…'}</button>
    </fieldset>
  </details>;
}
