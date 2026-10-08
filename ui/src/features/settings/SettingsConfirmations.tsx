import type { SettingsController } from './useSettingsController.js';

export function SettingsConfirmations({ controller }: { controller: SettingsController }) {
  const {
    confirmForget,
    confirmClear,
    setConfirmForget,
    setConfirmClear,
    confirmDialogRef,
    confirmCancelRef,
    onClose,
    onForgetActiveSource,
    run,
    backend,
    setDiagnostics,
    setNotice,
  } = controller;
  return <>        {confirmForget ? (
    <div className="settings-confirm-scrim" onMouseDown={() => setConfirmForget(null)}>
      <div
        ref={confirmDialogRef}
        className="settings-confirm"
        role="alertdialog"
        aria-modal="true"
        aria-label={`Forget ${confirmForget.name}`}
        onMouseDown={(event) => event.stopPropagation()}
      >
        <div className="settings-confirm__eyebrow">Local Library</div>
        <h2>Forget the active source?</h2>
        <p>
          MDReader will close this source and its open tabs, then remove its private index.
          The folder and every Markdown file stay untouched.
        </p>
        <div className="settings-confirm__actions">
          <button
            ref={confirmCancelRef}
            type="button"
            onClick={() => setConfirmForget(null)}
          >
            Cancel
          </button>
          <button
            type="button"
            className="settings-button--danger"
            onClick={() => {
              const source = confirmForget;
              setConfirmForget(null);
              onClose();
              void onForgetActiveSource(source);
            }}
          >
            Forget source
          </button>
        </div>
      </div>
    </div>
  ) : null}

    {confirmClear ? (
      <div className="settings-confirm-scrim" onMouseDown={() => setConfirmClear(false)}>
        <div
          ref={confirmDialogRef}
          className="settings-confirm"
          role="alertdialog"
          aria-modal="true"
          aria-label="Clear diagnostic logs"
          onMouseDown={(event) => event.stopPropagation()}
        >
          <div className="settings-confirm__eyebrow">Local action</div>
          <h2>Clear diagnostic logs?</h2>
          <p>
            This removes MDReader’s retained log files from this device. It cannot be undone.
          </p>
          <div className="settings-confirm__actions">
            <button
              ref={confirmCancelRef}
              type="button"
              onClick={() => setConfirmClear(false)}
            >
              Cancel
            </button>
            <button
              type="button"
              className="settings-button--danger"
              onClick={() => {
                setConfirmClear(false);
                void run('clear', () => backend.clearDiagnostics(true), (info) => {
                  setDiagnostics(info);
                  setNotice('Diagnostic logs cleared.');
                });
              }}
            >
              Clear diagnostic logs now
            </button>
          </div>
        </div>
      </div>
    ) : null}
  </>;
}
