import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { isTauri } from '../../shell/persistence';
import './startup-import-notice.css';

/** Read-only startup classification. Opening Settings never selects a source. */
export function StartupImportNotice({ onImport }: { onImport(): void }) {
  const [required, setRequired] = useState(false);
  useEffect(() => {
    if (!isTauri()) return;
    let current = true;
    void invoke<{ migrationRequired: boolean; usingImportedLibrary: boolean }>('native_profile_startup')
      .then((value) => {
        if (current && typeof value?.migrationRequired === 'boolean'
          && typeof value.usingImportedLibrary === 'boolean') setRequired(value.migrationRequired);
      })
      // Classification errors must not imply a completed or empty import.
      // Native startup remains authoritative about whether an owner can open.
      .catch(() => {});
    return () => { current = false; };
  }, []);
  if (!required) return null;
  return <section className="startup-import-notice" aria-labelledby="startup-import-title">
    <div>
      <h2 id="startup-import-title">Bring your existing library into the new reader</h2>
      <p>Import into a separate library. Your originals stay kept, and this window’s drafts can stay open.</p>
    </div>
    <button type="button" onClick={onImport}>Import existing library…</button>
  </section>;
}
