import { useCallback, useEffect, useRef, useState } from 'react';
import { settingsBackend, type CloudProvider, type CloudSourcesView, type ConnectedCatdbState, type DiagnosticInfo, type DiagnosticLevel, type LibrarySnapshot, type LibrarySourceDto } from '../../platform/settings.js';
import type { SettingsSheetProps } from './contracts.js';
import { errorMessage } from './presentation.js';

export function useSettingsController({
  backend = settingsBackend,
  localRoot = null,
  watcher,
  importState,
  onActivateSource,
  onForgetActiveSource,
  onSourceIntent = () => { },
  onLibraryChanged = () => { },
  onOpenProviderDocument,
  onClose,
}: SettingsSheetProps) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const closeRef = useRef<HTMLButtonElement>(null);
  const clearTriggerRef = useRef<HTMLButtonElement>(null);
  const confirmDialogRef = useRef<HTMLDivElement>(null);
  const confirmCancelRef = useRef<HTMLButtonElement>(null);
  const confirmWasOpenRef = useRef(false);
  const confirmTriggerRef = useRef<HTMLElement | null>(null);
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
  const [confirmForget, setConfirmForget] = useState<LibrarySourceDto | null>(null);


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
    if (confirmClear || confirmForget) {
      confirmWasOpenRef.current = true;
      confirmCancelRef.current?.focus();
    } else if (confirmWasOpenRef.current) {
      confirmWasOpenRef.current = false;
      confirmTriggerRef.current?.focus();
      confirmTriggerRef.current = null;
    }
  }, [confirmClear, confirmForget]);

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
    (source: LibrarySourceDto, trigger: HTMLElement) => {
      if (source.active) {
        confirmTriggerRef.current = trigger;
        setConfirmForget(source);
        return;
      }
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

  return {
    dialogRef,
    closeRef,
    clearTriggerRef,
    confirmDialogRef,
    confirmCancelRef,
    confirmTriggerRef,
    library,
    cloudSources,
    connectedCatdb,
    connectedCatdbOrigin,
    connectedCatdbBearer,
    diagnostics,
    busy,
    notice,
    failure,
    confirmClear,
    confirmForget,
    setConfirmClear,
    setConfirmForget,
    setConnectedCatdbOrigin,
    setConnectedCatdbBearer,
    setDiagnostics,
    setNotice,
    cloudControls,
    connectedCatdbControls,
    run,
    activate,
    addSource,
    forget,
    connectCloud,
    setCloudEnabled,
    disconnectCloud,
    enrollCatdb,
    setCatdbSourceEnabled,
    disconnectCatdb,
    setLevel,
    backend,
    localRoot,
    watcher,
    importState,
    onLibraryChanged,
    onOpenProviderDocument,
    onForgetActiveSource,
    onClose,
  };
}

export type SettingsController = ReturnType<typeof useSettingsController>;
