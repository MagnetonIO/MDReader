import { useEffect, useRef, useState } from 'react';
import { isTauri } from '../../shell/persistence';
import { unwrapNative, useReaderBackend } from '../../platform/backend';
import './startup-import-notice.css';

/** Read-only startup classification. Opening Settings never selects a source. */
export function StartupImportNotice({ onImport, onReturnController }: { onImport(): void; onReturnController?(): void }) {
  const backend = useReaderBackend();
  const [required, setRequired] = useState(false);
  const [returning, setReturning] = useState(false);
  const [unavailable, setUnavailable] = useState(false);
  const returnCallback = useRef(onReturnController);
  returnCallback.current = onReturnController;
  useEffect(() => {
    if (!isTauri()) return;
    let current = true;
    void unwrapNative(backend.profile.nativeProfileStartup())
      .then((value) => {
        if (current && typeof value?.migrationRequired === 'boolean'
          && typeof value.usingImportedLibrary === 'boolean') {
          setRequired(value.migrationRequired);
          setUnavailable(value.selectedLibraryUnavailable === true);
          if (value.returnController === true) {
            setReturning(true);
            returnCallback.current?.();
          }
        }
      })
      // Classification errors must not imply a completed or empty import.
      // Native startup remains authoritative about whether an owner can open.
      .catch(() => {});
    return () => { current = false; };
  }, [backend]);
  if (returning) return <section className="startup-import-notice" aria-labelledby="startup-return-title">
    <div><h2 id="startup-return-title">Return to your original library</h2>
      <p>This window keeps your return pending while the imported library closes normally.</p></div>
    <button type="button" onClick={onImport}>Open return controls</button>
  </section>;
  if (unavailable) return <section className="startup-import-notice" role="status" aria-labelledby="startup-unavailable-title">
    <div><h2 id="startup-unavailable-title">Your imported library couldn’t be opened</h2>
      <p>MDReader started with the standard library. Your libraries were kept; restart after reconnecting the folder to use it again.</p></div>
    <button type="button" onClick={() => setUnavailable(false)}>Dismiss</button>
  </section>;
  if (!required) return null;
  return <section className="startup-import-notice" aria-labelledby="startup-import-title">
    <div>
      <h2 id="startup-import-title">Bring your existing library into the new reader</h2>
      <p>Import into a separate library. Your originals stay kept, and this window’s drafts can stay open.</p>
    </div>
    <button type="button" onClick={onImport}>Import existing library…</button>
  </section>;
}
