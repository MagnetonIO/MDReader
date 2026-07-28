/**
 * Product Settings: the existing appearance preferences, backend-owned local
 * library membership, and bounded local diagnostics. The sheet owns presentation
 * only; switching still travels through AppShell's epoch/unsaved-edit machinery.
 */

import { useCallback, useEffect, useRef, useState } from 'react';

import type { ImportState } from '../platform/fs.js';
import {
  SettingsCommandError,
  settingsBackend,
  type CloudProvider,
  type CloudSourcesView,
  type ConnectedCatdbState,
  type DiagnosticInfo,
  type DiagnosticLevel,
  type LibrarySnapshot,
  type LibrarySourceDto,
  type SettingsBackend,
} from '../platform/settings.js';
import {
  TEXT_SCALE_STEPS,
  textScaleFactor,
  useSettingsStore,
} from '../stores/settings.js';
import type { WatcherStatus } from '../stores/sources.js';
import {
  CloseIcon,
  DropboxIcon,
  FolderIcon,
  GoogleDriveIcon,
} from './icons.js';

export interface SettingsSheetProps {
  backend?: SettingsBackend;
  watcher: WatcherStatus;
  importState: ImportState | null;
  onActivateSource: (source: LibrarySourceDto) => Promise<boolean>;
  onSourceIntent?: () => void;
  onLibraryChanged?: () => void;
  onClose: () => void;
}

const FOCUSABLE =
  'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

function readableBytes(bytes: number): string {
  if (bytes % 1024 === 0) return `${bytes / 1024} KiB`;
  return `${bytes.toLocaleString()} bytes`;
}

function errorMessage(cause: unknown): string {
  if (!(cause instanceof SettingsCommandError)) {
    return cause instanceof Error ? cause.message : 'The request could not be completed.';
  }
  const detail = cause.detail;
  if (!detail || typeof detail !== 'object' || !('kind' in detail)) {
    return 'The request could not be completed.';
  }
  switch (detail.kind) {
    case 'active-source':
      return 'The active source cannot be forgotten. Activate another source first.';
    case 'duplicate-root':
      return 'That folder is already in the library.';
    case 'corrupt-registry':
      return 'The saved library could not be read. Nothing was changed.';
    case 'unsupported-version':
      return 'This library was written by an unsupported MDReader version.';
    case 'invalid-root':
    case 'persist-failed':
    case 'cleanup-failed':
    case 'needs-native-repick':
      return 'MDReader could not finish the local library operation. Try again.';
    case 'corrupt-settings':
      return 'The local diagnostics settings could not be read.';
    case 'invalid-event':
      return 'The diagnostic event was rejected.';
    case 'write-failed':
      return 'MDReader could not write the local diagnostic log.';
    case 'export-failed':
      return 'MDReader could not export the local diagnostic bundle.';
    case 'clear-failed':
      return 'MDReader could not clear the local diagnostic logs.';
    case 'reveal-failed':
      return 'MDReader could not reveal the local diagnostics folder.';
    case 'confirmation-required':
      return 'Confirm before clearing diagnostic logs.';
    case 'source-not-found':
      return 'That source is no longer part of the library.';
    case 'source-unavailable':
      return 'That source is unavailable. Re-add the folder if it moved.';
    case 'grant-mismatch':
      return 'The source did not become active, so its restore preference was not changed.';
    case 'open-failed':
      return 'MDReader could not open that source.';
    case 'client-configuration-missing':
      return 'This MDReader build is missing the provider sign-in configuration.';
    case 'credential-unavailable':
      return 'The operating system credential store is unavailable.';
    case 'credential-missing':
      return 'Sign in again before enabling this source.';
    case 'source-paused':
      return 'Include this source in the Library before reading it.';
    case 'oauth-callback-unavailable':
      return 'MDReader could not open its local sign-in callback. Close another sign-in attempt and try again.';
    case 'oauth-state-mismatch':
    case 'oauth-invalid-callback':
      return 'The provider sign-in response could not be verified. Nothing was connected.';
    case 'oauth-denied':
      return 'The provider sign-in was cancelled or denied.';
    case 'oauth-timeout':
      return 'Provider sign-in timed out. Try again.';
    case 'oauth-transport':
    case 'provider-rejected':
      return 'The provider could not complete sign-in. Try again.';
    case 'query-invalid':
      return detail.detail;
    case 'query-failed':
      return `MDReader could not read that source. ${detail.detail}`;
    case 'query-not-found':
      return 'That provider document was not found. Refresh the source and try again.';
    case 'invalid-configuration':
    case 'rollback-failed':
      return 'MDReader could not safely update the cloud source. Nothing was connected.';
    case 'permission-denied':
      return 'That CatDB token does not grant one workspace with query and catalog access.';
    case 'connection-identity-changed':
      return 'The CatDB server identity changed. Disconnect it and verify the server before reconnecting.';
    case 'already-configured':
      return 'Disconnect the current CatDB workspace before connecting another.';
    case 'connection-not-found':
      return 'That CatDB workspace is no longer connected.';
    case 'no-sources-selected':
      return 'Choose at least one permitted CatDB source before querying.';
    case 'transport-unavailable':
    case 'timed-out':
      return 'The CatDB server could not be reached. Check its URL and try again.';
    case 'response-too-large':
    case 'invalid-response':
      return 'The CatDB server returned a response MDReader could not safely use.';
    case 'invalid-request':
      return 'The CatDB connection request was rejected before it left this device.';
    default:
      return 'The request could not be completed.';
  }
}

function sourceState(source: LibrarySourceDto): { label: string; unavailable: boolean } {
  switch (source.state.kind) {
    case 'available':
      return { label: 'Available', unavailable: false };
    case 'missing':
      return { label: 'Folder not found', unavailable: true };
    case 'unreadable':
      return { label: 'Folder cannot be read', unavailable: true };
    case 'not-directory':
      return { label: 'No longer a folder', unavailable: true };
  }
}

function activityLabel(watcher: WatcherStatus, importState: ImportState | null): string {
  if (importState?.phase === 'loading') return 'Loading local index model';
  if (importState?.phase === 'importing') {
    return `Indexing ${importState.filesDone} of ${importState.filesTotal} files`;
  }
  if (importState?.phase === 'reembedding') return 'Refreshing the local semantic index';
  if (importState?.phase === 'model-unavailable') return 'Readable; semantic index unavailable';
  if (importState?.phase === 'error') return 'Index needs attention';
  if (watcher === 'watching') return 'Watching for local changes';
  if (watcher === 'error') return 'Folder open; live updates unavailable';
  return 'Idle';
}

export function SettingsSheet({
  backend = settingsBackend,
  watcher,
  importState,
  onActivateSource,
  onSourceIntent = () => {},
  onLibraryChanged = () => {},
  onClose,
}: SettingsSheetProps) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const closeRef = useRef<HTMLButtonElement>(null);
  const clearTriggerRef = useRef<HTMLButtonElement>(null);
  const confirmDialogRef = useRef<HTMLDivElement>(null);
  const confirmCancelRef = useRef<HTMLButtonElement>(null);
  const confirmWasOpenRef = useRef(false);
  const operationSerials = useRef(new Map<string, number>());
  const invokingFocusRef = useRef<HTMLElement | null>(
    document.activeElement instanceof HTMLElement ? document.activeElement : null,
  );
  const [library, setLibrary] = useState<LibrarySnapshot | null>(null);
  const [cloudSources, setCloudSources] = useState<CloudSourcesView | null>(null);
  const [connectedCatdb, setConnectedCatdb] = useState<ConnectedCatdbState | null>(null);
  const [connectedCatdbOrigin, setConnectedCatdbOrigin] = useState('');
  const [connectedCatdbBearer, setConnectedCatdbBearer] = useState('');
  const [diagnostics, setDiagnostics] = useState<DiagnosticInfo | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const [confirmClear, setConfirmClear] = useState(false);

  const theme = useSettingsStore((state) => state.theme);
  const readingFont = useSettingsStore((state) => state.readingFont);
  const textScaleIndex = useSettingsStore((state) => state.textScaleIndex);

  useEffect(() => {
    closeRef.current?.focus();
    return () => {
      invokingFocusRef.current?.focus();
      for (const [key, serial] of operationSerials.current) {
        operationSerials.current.set(key, serial + 1);
      }
    };
  }, [backend]);

  useEffect(() => {
    if (!backend.connectedCatdbState) return;
    let live = true;
    void backend.connectedCatdbState()
      .then((state) => {
        if (live) setConnectedCatdb(state);
      })
      .catch((cause: unknown) => {
        if (live) setFailure(errorMessage(cause));
      });
    return () => {
      live = false;
    };
  }, [backend]);

  useEffect(() => {
    if (confirmClear) {
      confirmWasOpenRef.current = true;
      confirmCancelRef.current?.focus();
    } else if (confirmWasOpenRef.current) {
      confirmWasOpenRef.current = false;
      clearTriggerRef.current?.focus();
    }
  }, [confirmClear]);

  useEffect(() => {
    let live = true;
    void Promise.allSettled([backend.listSources(), backend.diagnosticsInfo()]).then(
      ([libraryResult, diagnosticsResult]) => {
        if (!live) return;
        if (libraryResult.status === 'fulfilled') setLibrary(libraryResult.value);
        else setFailure(errorMessage(libraryResult.reason));
        if (diagnosticsResult.status === 'fulfilled') setDiagnostics(diagnosticsResult.value);
        else setFailure((current) => current ?? errorMessage(diagnosticsResult.reason));
      },
    );
    return () => {
      live = false;
    };
  }, [backend]);

  const cloudControls =
    backend.listCloudSources &&
    backend.connectCloudSource &&
    backend.setCloudSourceEnabled &&
    backend.disconnectCloudSource
      ? {
          list: backend.listCloudSources,
          connect: backend.connectCloudSource,
          setEnabled: backend.setCloudSourceEnabled,
          disconnect: backend.disconnectCloudSource,
        }
      : null;
  const connectedCatdbControls =
    backend.connectedCatdbState &&
    backend.enrollConnectedCatdb &&
    backend.setConnectedCatdbSourceEnabled &&
    backend.disconnectConnectedCatdb
      ? {
          enroll: backend.enrollConnectedCatdb,
          setEnabled: backend.setConnectedCatdbSourceEnabled,
          disconnect: backend.disconnectConnectedCatdb,
        }
      : null;

  useEffect(() => {
    if (!cloudControls) return;
    let live = true;
    void cloudControls
      .list()
      .then((snapshot) => {
        if (live) setCloudSources(snapshot);
      })
      .catch((cause: unknown) => {
        if (live) setFailure(errorMessage(cause));
      });
    return () => {
      live = false;
    };
  }, [backend]);

  const run = useCallback(async <T,>(
    key: string,
    task: () => Promise<T>,
    apply: (value: T) => void,
    settled?: () => void,
  ) => {
    const serial = (operationSerials.current.get(key) ?? 0) + 1;
    operationSerials.current.set(key, serial);
    const current = () => operationSerials.current.get(key) === serial;
    setBusy(key);
    setFailure(null);
    setNotice(null);
    try {
      const value = await task();
      if (current()) apply(value);
    } catch (cause) {
      if (current()) setFailure(errorMessage(cause));
    } finally {
      if (current()) setBusy(null);
      settled?.();
    }
  }, []);

  const activate = useCallback(
    (source: LibrarySourceDto, intentAlreadyStarted = false) => {
      if (!intentAlreadyStarted) onSourceIntent();
      onClose();
      void onActivateSource(source);
    },
    [onActivateSource, onClose, onSourceIntent],
  );

  const addSource = useCallback(() => {
    onSourceIntent();
    void run(
      'add',
      () => backend.pickAndAddSource(),
      (picked) => {
        if (!picked.source) return;
        activate(picked.source, true);
      },
      onLibraryChanged,
    );
  }, [activate, backend, onLibraryChanged, onSourceIntent, run]);

  const forget = useCallback(
    (source: LibrarySourceDto) => {
      onSourceIntent();
      void run(
        `forget:${source.id}`,
        () => backend.forgetSource(source.id),
        setLibrary,
        onLibraryChanged,
      );
    },
    [backend, onLibraryChanged, onSourceIntent, run],
  );

  const connectCloud = useCallback(
    (provider: CloudProvider) => {
      if (!cloudControls) return;
      void run(
        `connect:${provider}`,
        () => cloudControls.connect(provider),
        setCloudSources,
        onLibraryChanged,
      );
    },
    [cloudControls, onLibraryChanged, run],
  );

  const setCloudEnabled = useCallback(
    (provider: CloudProvider, enabled: boolean) => {
      if (!cloudControls) return;
      void run(
        `toggle:${provider}`,
        () => cloudControls.setEnabled(provider, enabled),
        setCloudSources,
        onLibraryChanged,
      );
    },
    [cloudControls, onLibraryChanged, run],
  );

  const disconnectCloud = useCallback(
    (provider: CloudProvider) => {
      if (!cloudControls) return;
      void run(
        `disconnect:${provider}`,
        () => cloudControls.disconnect(provider),
        (snapshot) => {
          setCloudSources(snapshot);
        },
        onLibraryChanged,
      );
    },
    [cloudControls, onLibraryChanged, run],
  );

  const enrollCatdb = useCallback(() => {
    if (!connectedCatdbControls) return;
    const bearer = connectedCatdbBearer;
    setConnectedCatdbBearer('');
    void run(
      'connected-catdb:enroll',
      () => connectedCatdbControls.enroll(connectedCatdbOrigin, bearer),
      setConnectedCatdb,
      onLibraryChanged,
    );
  }, [
    connectedCatdbBearer,
    connectedCatdbControls,
    connectedCatdbOrigin,
    onLibraryChanged,
    run,
  ]);

  const setCatdbSourceEnabled = useCallback(
    (connectionId: string, sourceId: string, enabled: boolean) => {
      if (!connectedCatdbControls) return;
      void run(
        `connected-catdb:source:${sourceId}`,
        () => connectedCatdbControls.setEnabled(connectionId, sourceId, enabled),
        setConnectedCatdb,
        onLibraryChanged,
      );
    },
    [connectedCatdbControls, onLibraryChanged, run],
  );

  const disconnectCatdb = useCallback(
    (connectionId: string) => {
      if (!connectedCatdbControls) return;
      void run(
        'connected-catdb:disconnect',
        () => connectedCatdbControls.disconnect(connectionId),
        setConnectedCatdb,
        onLibraryChanged,
      );
    },
    [connectedCatdbControls, onLibraryChanged, run],
  );

  const setLevel = useCallback(
    (level: DiagnosticLevel) => {
      void run(`level:${level}`, () => backend.setDiagnosticLevel(level), setDiagnostics);
    },
    [backend, run],
  );

  const onKeyDown = useCallback(
    (event: React.KeyboardEvent<HTMLDivElement>) => {
      if (event.key === 'Escape') {
        event.preventDefault();
        event.stopPropagation();
        if (confirmClear) setConfirmClear(false);
        else onClose();
        return;
      }
      if (event.key !== 'Tab') return;
      const focusScope = confirmClear ? confirmDialogRef.current : dialogRef.current;
      const focusable = Array.from(
        focusScope?.querySelectorAll<HTMLElement>(FOCUSABLE) ?? [],
      );
      if (focusable.length === 0) return;
      const first = focusable[0]!;
      const last = focusable.at(-1)!;
      if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      } else if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      }
    },
    [confirmClear, onClose],
  );

  return (
    <div className="settings-backdrop" role="presentation" onMouseDown={onClose}>
      <div
        ref={dialogRef}
        className="settings-sheet"
        role="dialog"
        aria-modal="true"
        aria-label="Settings"
        onKeyDown={onKeyDown}
        onMouseDown={(event) => event.stopPropagation()}
      >
        <header className="settings-sheet__header">
          <div>
            <div className="settings-sheet__eyebrow">MDReader</div>
            <h1 className="settings-sheet__title">Settings</h1>
          </div>
          <button
            ref={closeRef}
            type="button"
            className="settings-sheet__close"
            aria-label="Close Settings"
            onClick={onClose}
          >
            <CloseIcon />
          </button>
        </header>

        <div className="settings-sheet__scroll">
          <section className="settings-section" aria-labelledby="settings-appearance">
            <div className="settings-section__intro">
              <div>
                <div className="settings-section__number">01</div>
                <h2 id="settings-appearance">Appearance</h2>
              </div>
              <p>Shape the reading room. These preferences use the same store as the titlebar.</p>
            </div>

            <div className="settings-field">
              <div className="settings-field__label">Theme</div>
              <div className="settings-segment" aria-label="Theme">
                {(['system', 'light', 'dark'] as const).map((value) => (
                  <button
                    key={value}
                    type="button"
                    aria-pressed={theme === value}
                    onClick={() => useSettingsStore.getState().setTheme(value)}
                  >
                    {value[0]!.toUpperCase() + value.slice(1)}
                  </button>
                ))}
              </div>
            </div>

            <div className="settings-field">
              <div>
                <div className="settings-field__label">Reading face</div>
                <div className="settings-field__hint">Only document typography changes.</div>
              </div>
              <div className="settings-segment" aria-label="Reading font">
                {(['serif', 'sans'] as const).map((value) => (
                  <button
                    key={value}
                    type="button"
                    aria-pressed={readingFont === value}
                    onClick={() => useSettingsStore.getState().setReadingFont(value)}
                  >
                    {value[0]!.toUpperCase() + value.slice(1)}
                  </button>
                ))}
              </div>
            </div>

            <div className="settings-field">
              <div>
                <div className="settings-field__label">Text scale</div>
                <div className="settings-field__hint">
                  {Math.round(textScaleFactor({ textScaleIndex }) * 100)}%
                </div>
              </div>
              <div className="settings-stepper">
                <button
                  type="button"
                  aria-label="Decrease text size"
                  disabled={textScaleIndex === 0}
                  onClick={() => useSettingsStore.getState().decreaseTextSize()}
                >
                  −
                </button>
                <span aria-hidden="true">{TEXT_SCALE_STEPS[textScaleIndex] ?? 1}</span>
                <button
                  type="button"
                  aria-label="Increase text size"
                  disabled={textScaleIndex === TEXT_SCALE_STEPS.length - 1}
                  onClick={() => useSettingsStore.getState().increaseTextSize()}
                >
                  +
                </button>
              </div>
            </div>
          </section>

          <section className="settings-section" aria-labelledby="settings-library">
            <div className="settings-section__intro">
              <div>
                <div className="settings-section__number">02</div>
                <h2 id="settings-library">Library</h2>
              </div>
              <p>
                Add the Markdown sources you want to read and search as one Library.
              </p>
            </div>

            <div className="settings-callout">
              <FolderIcon />
              <div>
                <strong>Local Markdown folders</strong>
                <span>Membership is remembered; your files stay where you put them.</span>
              </div>
              <button type="button" disabled={busy !== null} onClick={addSource}>
                {busy === 'add' ? 'Choosing…' : 'Add folder'}
              </button>
            </div>

            <div className="library-list" aria-live="polite">
              {!library && !failure ? <div className="settings-empty">Reading local library…</div> : null}
              {library?.sources.length === 0 ? (
                <div className="settings-empty">No folders yet. Add one to begin.</div>
              ) : null}
              {library?.sources.map((source) => {
                const state = sourceState(source);
                return (
                  <article
                    key={source.id}
                    className={`library-source${source.active ? ' library-source--active' : ''}`}
                  >
                    <div className="library-source__rail" aria-hidden="true" />
                    <div className="library-source__body">
                      <div className="library-source__heading">
                        <div>
                          <h3>{source.name}</h3>
                          <div className="library-source__path" title={source.path}>
                            {source.path}
                          </div>
                        </div>
                        {source.active ? <span className="settings-badge">Active</span> : null}
                      </div>
                      <div className="library-source__meta">
                        <span className={state.unavailable ? 'settings-state--warning' : ''}>
                          {state.label}
                        </span>
                        <span>
                          {source.index_state === 'present'
                            ? 'Index on disk'
                            : source.index_state === 'not-created'
                              ? 'Index not created'
                              : 'Index status unavailable'}
                        </span>
                        {source.active ? <span>{activityLabel(watcher, importState)}</span> : null}
                      </div>
                      <div className="library-source__actions">
                        {!source.active ? (
                          <button
                            type="button"
                            aria-label={`Activate ${source.name}`}
                            disabled={state.unavailable || busy !== null}
                            onClick={() => activate(source)}
                          >
                            Activate
                          </button>
                        ) : null}
                        <button
                          type="button"
                          className="settings-button--quiet"
                          aria-label={`Forget ${source.name}`}
                          disabled={source.active || busy !== null}
                          title={
                            source.active
                              ? 'Activate another source before forgetting this one'
                              : 'Removes this entry and its local MDReader index; your files stay'
                          }
                          onClick={() => forget(source)}
                        >
                          {busy === `forget:${source.id}` ? 'Forgetting…' : 'Forget'}
                        </button>
                      </div>
                    </div>
                  </article>
                );
              })}
            </div>
            {library?.cleanup.kind === 'pending' ? (
              <p className="settings-footnote settings-state--warning" role="status">
                Finishing local cleanup for {library.cleanup.source_count}{' '}
                {library.cleanup.source_count === 1 ? 'source' : 'sources'}. Your folders are not
                affected.
              </p>
            ) : null}
            <p className="settings-footnote">
              Forget removes the library entry and MDReader’s private index. It never deletes the
              folder or its files.
            </p>

            {cloudControls ? (
              <div className="cloud-sources">
                <div className="cloud-sources__heading">
                  <div>
                    <h3>Connected accounts</h3>
                    <p>
                      Sign in through MDReader’s registered apps. Credentials stay in the
                      operating system’s secure storage.
                    </p>
                  </div>
                </div>
                <div className="cloud-sources__list" aria-live="polite">
                  {!cloudSources && !failure ? (
                    <div className="settings-empty">Reading connected accounts…</div>
                  ) : null}
                  {cloudSources?.sources.map((source) => {
                    const connected = source.connection !== 'disconnected';
                    const reconnect = source.connection === 'reauthorize';
                    return (
                      <article
                        className="cloud-source cloud-source--provider"
                        key={source.provider}
                      >
                        <div className="cloud-source__icon">
                          {source.provider === 'google-drive' ? (
                            <GoogleDriveIcon />
                          ) : (
                            <DropboxIcon />
                          )}
                        </div>
                        <div className="cloud-source__copy">
                          <div className="cloud-source__title">
                            <h4>{source.label}</h4>
                            {connected ? (
                              <span className={reconnect ? 'settings-state--warning' : ''}>
                                {reconnect ? 'Sign in again' : source.enabled ? 'Included' : 'Paused'}
                              </span>
                            ) : null}
                          </div>
                          <p>
                            {source.provider === 'google-drive'
                              ? 'Choose the Drive files or folders that belong in this Library.'
                              : 'Read Markdown and text files from your Dropbox account.'}
                          </p>
                        </div>
                        <div className="cloud-source__actions">
                          {connected ? (
                            <>
                              {!reconnect ? (
                                <button
                                  type="button"
                                  className="source-switch"
                                  role="switch"
                                  aria-label={`Include ${source.label} in Library`}
                                  aria-checked={source.enabled}
                                  disabled={busy !== null}
                                  onClick={() =>
                                    setCloudEnabled(source.provider, !source.enabled)
                                  }
                                >
                                  <span aria-hidden="true" />
                                </button>
                              ) : (
                                <button
                                  type="button"
                                  disabled={busy !== null}
                                  aria-label={`Reconnect ${source.label}`}
                                  onClick={() => connectCloud(source.provider)}
                                >
                                  Reconnect
                                </button>
                              )}
                              <button
                                type="button"
                                className="settings-button--quiet"
                                disabled={busy !== null}
                                aria-label={`Disconnect ${source.label}`}
                                onClick={() => disconnectCloud(source.provider)}
                              >
                                {busy === `disconnect:${source.provider}`
                                  ? 'Disconnecting…'
                                  : 'Disconnect'}
                              </button>
                            </>
                          ) : (
                            <button
                              type="button"
                              disabled={busy !== null}
                              aria-label={`Connect ${source.label}`}
                              onClick={() => connectCloud(source.provider)}
                            >
                              {busy === `connect:${source.provider}` ? 'Connecting…' : 'Connect'}
                            </button>
                          )}
                        </div>
                      </article>
                    );
                  })}
                </div>
                <p className="settings-footnote">
                  Pausing keeps the account connected. Disconnecting removes its saved credentials.
                  Neither action changes files in Drive or Dropbox.
                </p>
              </div>
            ) : null}
            {connectedCatdb && connectedCatdbControls ? (
              <div className="cloud-sources">
                <div className="cloud-sources__heading">
                  <div>
                    <h3>Connected CatDB</h3>
                    <p>
                      Sign in to a CatDB workspace to choose the database and warehouse sources
                      your account can read.
                    </p>
                  </div>
                </div>
                {library?.connected_catdb_rediscovery_required ? (
                  <p className="settings-state--warning" role="status">
                    This Library’s previous CatDB connection did not record a verified schema
                    version. Connect it again to rediscover and pin the current schema before
                    querying.
                  </p>
                ) : null}
                {connectedCatdb.state === 'disconnected' ? (
                  <article className="cloud-source connected-catdb-card">
                    <div className="connected-catdb-form">
                      <label>
                        <span>CatDB server URL</span>
                        <input
                          type="url"
                          inputMode="url"
                          autoComplete="url"
                          value={connectedCatdbOrigin}
                          placeholder="https://catdb.example.com"
                          disabled={busy !== null}
                          onChange={(event) => setConnectedCatdbOrigin(event.currentTarget.value)}
                        />
                      </label>
                      <label>
                        <span>CatDB bearer token</span>
                        <input
                          type="password"
                          autoComplete="off"
                          value={connectedCatdbBearer}
                          disabled={busy !== null}
                          onChange={(event) => setConnectedCatdbBearer(event.currentTarget.value)}
                        />
                      </label>
                      <p>
                        Direct database credentials are never stored in MDReader. The bearer is
                        written once to the operating system credential store and cleared here.
                      </p>
                    </div>
                    <div className="cloud-source__actions">
                      <button
                        type="button"
                        disabled={
                          busy !== null ||
                          connectedCatdbOrigin.trim().length === 0 ||
                          connectedCatdbBearer.length === 0
                        }
                        aria-label="Connect CatDB workspace"
                        onClick={enrollCatdb}
                      >
                        {busy === 'connected-catdb:enroll' ? 'Connecting…' : 'Connect'}
                      </button>
                    </div>
                  </article>
                ) : (
                  <>
                    <article className="cloud-source">
                      <div className="cloud-source__copy">
                        <div className="cloud-source__title">
                          <h4>{connectedCatdb.label}</h4>
                          <span>Connected</span>
                        </div>
                        <p>
                          Workspace {connectedCatdb.workspace_id} · catalog{' '}
                          {connectedCatdb.catalog_revision_id}
                        </p>
                      </div>
                      <div className="cloud-source__actions">
                        <button
                          type="button"
                          className="settings-button--quiet"
                          disabled={busy !== null}
                          aria-label={`Disconnect ${connectedCatdb.label}`}
                          onClick={() => disconnectCatdb(connectedCatdb.connection_id)}
                        >
                          {busy === 'connected-catdb:disconnect'
                            ? 'Disconnecting…'
                            : 'Disconnect'}
                        </button>
                      </div>
                    </article>
                    <div className="cloud-sources__list" aria-live="polite">
                      {connectedCatdb.sources.length === 0 ? (
                        <div className="settings-empty">
                          This workspace exposes no permitted sources.
                        </div>
                      ) : null}
                      {connectedCatdb.sources.map((source) => {
                        const label = source.connector_id ?? source.source_id;
                        return (
                          <article className="cloud-source" key={source.source_id}>
                            <div className="cloud-source__copy">
                              <div className="cloud-source__title">
                                <h4>{label}</h4>
                                {!source.runtime_bound ? (
                                  <span className="settings-state--warning">Unavailable</span>
                                ) : source.enabled ? (
                                  <span>Included</span>
                                ) : (
                                  <span>Paused</span>
                                )}
                              </div>
                              <p>Source {source.source_id}</p>
                            </div>
                            <div className="cloud-source__actions">
                              <button
                                type="button"
                                className="source-switch"
                                role="switch"
                                aria-label={`Include ${label} in Connected CatDB queries`}
                                aria-checked={source.enabled}
                                disabled={!source.runtime_bound || busy !== null}
                                onClick={() =>
                                  setCatdbSourceEnabled(
                                    connectedCatdb.connection_id,
                                    source.source_id,
                                    !source.enabled,
                                  )
                                }
                              >
                                <span aria-hidden="true" />
                              </button>
                            </div>
                          </article>
                        );
                      })}
                    </div>
                    <p className="settings-footnote">
                      Queries are pinned natively to this workspace, catalog revision, and the
                      enabled sources above. Direct database credentials are never stored in
                      MDReader; they remain server-managed.
                    </p>
                  </>
                )}
              </div>
            ) : null}
          </section>

          <section className="settings-section" aria-labelledby="settings-diagnostics">
            <div className="settings-section__intro">
              <div>
                <div className="settings-section__number">03</div>
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
                    onClick={() => setConfirmClear(true)}
                  >
                    Clear logs
                  </button>
                </div>
              </div>
            ) : !failure ? (
              <div className="settings-empty">Reading diagnostics settings…</div>
            ) : null}
          </section>
        </div>

        {failure ? (
          <div className="settings-sheet__message settings-sheet__message--error" role="alert">
            {failure}
          </div>
        ) : null}
        {notice ? (
          <div className="settings-sheet__message" role="status">
            {notice}
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
              <div className="settings-section__number">Local action</div>
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
      </div>
    </div>
  );
}
