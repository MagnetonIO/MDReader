import { readableBytes } from './presentation.js';
import type { SettingsController } from './useSettingsController.js';

export function DiagnosticSettings({ controller }: { controller: SettingsController }) {
  const {
    diagnostics,
    busy,
    setLevel,
    backend,
    run,
    setNotice,
    clearTriggerRef,
    confirmTriggerRef,
    setConfirmClear,
    failure,
  } = controller;
  return (<section className="settings-section" aria-labelledby="settings-diagnostics">
    <div className="settings-section__intro">
      <div>
        <h2 id="settings-diagnostics">Diagnostics</h2>
      </div>
      <p>
        Coarse lifecycle logs stay on this device. There is no upload or telemetry path.
      </p>
    </div>

    <div className="settings-field settings-field--stack">
      <div>
        <div className="settings-field__label">Detail</div>
        <div className="settings-field__hint">Changing detail affects future events.</div>
      </div>
      <div className="settings-segment settings-segment--wide" aria-label="Diagnostic detail">
        {(
          [
            ['errors', 'Errors only'],
            ['standard', 'Standard'],
            ['verbose', 'Verbose'],
          ] as const
        ).map(([level, label]) => (
          <button
            key={level}
            type="button"
            aria-pressed={diagnostics?.level === level}
            disabled={!diagnostics || busy !== null}
            onClick={() => setLevel(level)}
          >
            {label}
          </button>
        ))}
      </div>
    </div>

    {diagnostics ? (
      <div className="diagnostic-card">
        <div className="diagnostic-card__location">Stored locally on this device</div>
        <div className="diagnostic-card__bounds">
          {readableBytes(diagnostics.max_file_bytes)} per file ·{' '}
          up to {diagnostics.max_retained_files}{' '}
          {diagnostics.max_retained_files === 1 ? 'file' : 'files'} ·{' '}
          {readableBytes(diagnostics.max_export_bytes)} export
        </div>
        <div className="diagnostic-card__actions">
          <button
            type="button"
            disabled={busy !== null}
            onClick={() =>
              void run('reveal', () => backend.revealDiagnostics(), () =>
                setNotice('Opened the local diagnostics folder.'),
              )
            }
          >
            Reveal
          </button>
          <button
            type="button"
            disabled={busy !== null}
            onClick={() =>
              void run('export', () => backend.exportDiagnostics(), (result) => {
                setNotice(
                  result.status === 'exported'
                    ? `Exported ${readableBytes(result.bytes_written)}.`
                    : 'Export cancelled.',
                );
              })
            }
          >
            Export…
          </button>
          <button
            ref={clearTriggerRef}
            type="button"
            className="settings-button--danger"
            disabled={busy !== null}
            onClick={() => {
              confirmTriggerRef.current = clearTriggerRef.current;
              setConfirmClear(true);
            }}
          >
            Clear logs
          </button>
        </div>
      </div>
    ) : !failure ? (
      <div className="settings-empty">Reading diagnostics settings…</div>
    ) : null}
  </section>
  );
}
