import { useCallback, useEffect, useMemo, useRef, useState } from 'react';

import { applyTheme, useSettingsStore } from '../stores/settings';
import type { SettingsSnapshot } from '../stores/settings';
import type { TabState } from '../stores/types';
import { loadPersisted, saveSettings } from '../shell/persistence';
import { MobileReader } from './MobileReader';
import {
  createEnterpriseMobileSourceAdapter,
  type EnterpriseMobileSourceAdapter,
  type MobileLibraryDocument,
  type MobileLibraryResult,
} from './mobile-source-adapter';
import {
  MOBILE_SOURCE_ALLOWLIST,
  loadEnterpriseServerConfig,
  saveEnterpriseServerConfig,
  validateEnterpriseServerConfig,
  type EnterpriseServerConfig,
} from './source-policy';
import './mobile.css';

type MobileTab = 'library' | 'search' | 'sources' | 'settings';

export interface MobileAppShellProps {
  platform: 'ios' | 'android';
}

const NAVIGATION: ReadonlyArray<{ id: MobileTab; label: string }> = [
  { id: 'library', label: 'Library' },
  { id: 'search', label: 'Search' },
  { id: 'sources', label: 'Sources' },
  { id: 'settings', label: 'Settings' },
];

type MobileSourceState =
  | { status: 'loading' }
  | { status: 'not-configured' }
  | MobileLibraryResult;

function useMediaQuery(query: string, fallback: boolean): boolean {
  const [matched, setMatched] = useState(() =>
    typeof window.matchMedia === 'function' ? window.matchMedia(query).matches : fallback,
  );
  useEffect(() => {
    if (typeof window.matchMedia !== 'function') return;
    const media = window.matchMedia(query);
    const update = (event: MediaQueryListEvent) => setMatched(event.matches);
    setMatched(media.matches);
    media.addEventListener('change', update);
    return () => media.removeEventListener('change', update);
  }, [query]);
  return matched;
}

function currentSettings(): SettingsSnapshot {
  const settings = useSettingsStore.getState();
  return {
    theme: settings.theme,
    readingFont: settings.readingFont,
    textScaleIndex: settings.textScaleIndex,
    sidebarCollapsed: settings.sidebarCollapsed,
    outlineVisible: settings.outlineVisible,
    relatedRailOpen: settings.relatedRailOpen,
    inspectorOpen: settings.inspectorOpen,
    lensSchema: settings.lensSchema,
    reviewer: settings.reviewer,
  };
}

function sourceStatus(state: MobileSourceState): string {
  if (state.status === 'loading') return 'Loading the admitted Enterprise library…';
  if (state.status === 'not-configured') return 'No mobile source is configured.';
  if (state.status === 'ready') {
    return `Connected to ${state.label} · ${state.documents.length} document${state.documents.length === 1 ? '' : 's'}`;
  }
  return state.message;
}

function LibraryScreen({
  state,
  onOpen,
}: {
  state: MobileSourceState;
  onOpen: (document: MobileLibraryDocument) => void;
}) {
  const files = state.status === 'ready' ? state.documents : [];
  return (
    <section className="mobile-screen" aria-label="Library">
      <div className="mobile-screen__head">
        <span className="mobile-eyebrow">Library</span>
        <h1>{state.status === 'ready' ? state.label : 'Your documents'}</h1>
      </div>
      {state.status !== 'ready' ? (
        <div className="mobile-state" role="status">
          <h2>Library unavailable</h2>
          <p>{sourceStatus(state)}</p>
        </div>
      ) : files.length === 0 ? (
        <div className="mobile-state">
          <h2>No documents available</h2>
          <p>The admitted source contains no available documents.</p>
        </div>
      ) : (
        <ol className="mobile-document-list">
          {files.map((file) => (
            <li key={`${file.source.id}:${file.sourceRoot.version}:${file.document.id}`}>
              <button
                type="button"
                className="mobile-document"
                aria-label={file.name}
                onClick={() => onOpen(file)}
              >
                <span>
                  <strong>{file.title}</strong>
                  <small>{file.name}</small>
                </span>
                <small>{Math.max(1, Math.ceil(file.size / 1024))} KiB</small>
              </button>
            </li>
          ))}
        </ol>
      )}
    </section>
  );
}

function SearchScreen({
  documents,
  onOpen,
}: {
  documents: MobileLibraryDocument[];
  onOpen: (document: MobileLibraryDocument) => void;
}) {
  const [query, setQuery] = useState('');
  const results = useMemo(() => {
    const needle = query.trim().toLocaleLowerCase();
    if (!needle) return [];
    return documents
      .filter(
        (file) =>
          file.name.toLocaleLowerCase().includes(needle) ||
          file.title.toLocaleLowerCase().includes(needle),
      )
      .slice(0, 50);
  }, [documents, query]);

  return (
    <section className="mobile-screen" aria-label="Search">
      <div className="mobile-screen__head">
        <span className="mobile-eyebrow">Library search</span>
        <h1>Find a document</h1>
      </div>
      <label className="mobile-field">
        <span>Search</span>
        <input
          type="search"
          value={query}
          onChange={(event) => setQuery(event.currentTarget.value)}
          placeholder="Title or filename"
        />
      </label>
      {query && results.length === 0 ? (
        <p className="mobile-state">No matching documents in the available library.</p>
      ) : (
        <ol className="mobile-document-list">
          {results.map((file) => (
            <li key={`${file.source.id}:${file.sourceRoot.version}:${file.document.id}`}>
              <button
                type="button"
                className="mobile-document"
                aria-label={file.name}
                onClick={() => onOpen(file)}
              >
                <span>{file.title}</span>
                <small>{file.name}</small>
              </button>
            </li>
          ))}
        </ol>
      )}
    </section>
  );
}

function SourcesScreen({
  savedConfig,
  sourceState,
  onConnect,
}: {
  savedConfig: EnterpriseServerConfig | null;
  sourceState: MobileSourceState;
  onConnect: (config: EnterpriseServerConfig) => Promise<MobileLibraryResult>;
}) {
  const [message, setMessage] = useState<string | null>(null);
  const [serverError, setServerError] = useState<string | null>(null);
  const [serverUrl, setServerUrl] = useState(savedConfig?.serverUrl ?? '');
  const [sessionReference, setSessionReference] = useState(
    savedConfig?.sessionReference ?? '',
  );

  useEffect(() => {
    if (!savedConfig) return;
    setServerUrl(savedConfig.serverUrl);
    setSessionReference(savedConfig.sessionReference);
  }, [savedConfig]);

  return (
    <section className="mobile-screen" aria-label="Sources">
      <div className="mobile-screen__head">
        <span className="mobile-eyebrow">Read-only membership</span>
        <h1>Sources</h1>
      </div>
      <div className="mobile-source-list">
        <article className="mobile-source">
          <h2>Device folders</h2>
          <p className="mobile-source__status">
            Unavailable until a durable mobile folder grant is implemented
          </p>
          <p>MDReader will not accept an unscoped path or substitute a file selection.</p>
        </article>
        {MOBILE_SOURCE_ALLOWLIST.filter(
          (source) => source.connection === 'secure-vault-unavailable',
        ).map((source) => (
          <article className="mobile-source" key={source.kind}>
            <h2>{source.label}</h2>
            <p className="mobile-source__status">
              Unavailable on mobile until secure sign-in is supported
            </p>
            <p>Tokens are never stored in mobile source configuration.</p>
          </article>
        ))}
        <article className="mobile-source">
          <h2>Enterprise CatDB server</h2>
          <p>Connect to an authenticated CatDB service. Warehouse credentials stay on the server.</p>
          <p className="mobile-source__status" role="status">
            {sourceStatus(sourceState)}
          </p>
          <label className="mobile-field">
            <span>Server URL</span>
            <input
              type="url"
              value={serverUrl}
              onChange={(event) => setServerUrl(event.currentTarget.value)}
              inputMode="url"
              autoCapitalize="none"
            />
          </label>
          <label className="mobile-field">
            <span>Session reference</span>
            <input
              value={sessionReference}
              onChange={(event) => setSessionReference(event.currentTarget.value)}
              autoCapitalize="none"
              autoComplete="off"
            />
          </label>
          <button
            type="button"
            className="mobile-action mobile-touch"
            onClick={() => {
              const result = validateEnterpriseServerConfig(serverUrl, sessionReference);
              if (result.status === 'error') {
                setServerError(result.message);
                return;
              }
              setServerError(null);
              void saveEnterpriseServerConfig(result.config)
                .then(() => onConnect(result.config))
                .then((connection) =>
                  setMessage(
                    connection.status === 'ready'
                      ? 'Server boundary saved and connected.'
                      : `Server boundary saved. ${sourceStatus(connection)}`,
                  ),
                )
                .catch(() => setServerError('The server boundary could not be saved on this device.'));
            }}
          >
            Use server
          </button>
          {serverError ? <p role="alert">{serverError}</p> : null}
        </article>
      </div>
      {message ? <p role="status">{message}</p> : null}
    </section>
  );
}

function SettingsScreen() {
  const theme = useSettingsStore((state) => state.theme);
  const setTheme = useSettingsStore((state) => state.setTheme);
  return (
    <section className="mobile-screen" aria-label="Settings">
      <div className="mobile-screen__head">
        <span className="mobile-eyebrow">On this device</span>
        <h1>Settings</h1>
      </div>
      <fieldset className="mobile-theme">
        <legend>Appearance</legend>
        {(['system', 'light', 'dark'] as const).map((choice) => (
          <label key={choice}>
            <input
              type="radio"
              name="mobile-theme"
              checked={theme === choice}
              onChange={() => setTheme(choice)}
            />
            <span>{choice[0]!.toUpperCase() + choice.slice(1)}</span>
          </label>
        ))}
      </fieldset>
      <div className="mobile-source">
        <h2>Offline copies</h2>
        <p>Cached documents remain source-attributed and never imply a complete library.</p>
      </div>
    </section>
  );
}

export function MobileAppShell({ platform }: MobileAppShellProps) {
  const [tab, setTab] = useState<MobileTab>('library');
  const [readerMode, setReaderMode] = useState<'reader' | 'inspector'>('reader');
  const [hydrated, setHydrated] = useState(false);
  const [sourceState, setSourceState] = useState<MobileSourceState>({ status: 'loading' });
  const [savedConfig, setSavedConfig] = useState<EnterpriseServerConfig | null>(null);
  const [adapter, setAdapter] = useState<EnterpriseMobileSourceAdapter | null>(null);
  const [activeDocument, setActiveDocument] = useState<MobileLibraryDocument | null>(null);
  const sourceEpoch = useRef(0);
  const tablet = useMediaQuery('(min-width: 768px)', window.innerWidth >= 768);
  const systemDark = useMediaQuery('(prefers-color-scheme: dark)', false);
  const theme = useSettingsStore((state) => state.theme);

  useEffect(() => applyTheme(theme), [theme]);

  const connect = useCallback(async (config: EnterpriseServerConfig) => {
    const epoch = ++sourceEpoch.current;
    const nextAdapter = createEnterpriseMobileSourceAdapter(config);
    setSavedConfig(config);
    setSourceState({ status: 'loading' });
    const result = await nextAdapter.listDocuments();
    if (sourceEpoch.current !== epoch) return result;
    setAdapter(result.status === 'ready' ? nextAdapter : null);
    setActiveDocument(null);
    setSourceState(result);
    return result;
  }, []);

  useEffect(() => {
    let current = true;
    void Promise.all([loadPersisted(), loadEnterpriseServerConfig()]).then(
      async ([persisted, config]) => {
        if (!current) return;
        if (Object.keys(persisted.settings).length > 0) {
          useSettingsStore.getState().hydrate(persisted.settings);
        }
        setHydrated(true);
        if (config) await connect(config);
        else if (current) setSourceState({ status: 'not-configured' });
      },
    );
    return () => {
      current = false;
      sourceEpoch.current += 1;
    };
  }, [connect]);

  useEffect(() => {
    if (!hydrated) return;
    const handle = window.setTimeout(() => void saveSettings(currentSettings()), 100);
    return () => window.clearTimeout(handle);
  }, [hydrated, theme]);

  useEffect(() => {
    if (!hydrated) return;
    const flush = () => void saveSettings(currentSettings());
    const onVisibility = () => {
      if (document.visibilityState === 'hidden') flush();
    };
    window.addEventListener('beforeunload', flush);
    document.addEventListener('visibilitychange', onVisibility);
    return () => {
      window.removeEventListener('beforeunload', flush);
      document.removeEventListener('visibilitychange', onVisibility);
    };
  }, [hydrated]);

  const documents = sourceState.status === 'ready' ? sourceState.documents : [];
  const openDocument = (document: MobileLibraryDocument) => {
    setActiveDocument(document);
    setReaderMode('reader');
  };
  const closeReader = () => {
    setActiveDocument(null);
    setReaderMode('reader');
  };
  const activeTab = useMemo<TabState | null>(
    () =>
      activeDocument
        ? {
            id: `mobile:${activeDocument.source.kind}:${activeDocument.source.id}:${activeDocument.document.id}`,
            docRef: {
              path: activeDocument.document.path,
              name: activeDocument.name,
            },
            scroll: { block: 0, offset: 0 },
            history: [
              {
                path: activeDocument.document.path,
                name: activeDocument.name,
              },
            ],
            historyIndex: 0,
            pinned: false,
            originRoot: activeDocument.sourceRoot.id,
          }
        : null,
    [activeDocument],
  );
  const readActiveDocument = useCallback(() => {
    if (!adapter || !activeDocument) {
      return Promise.reject(new Error('No admitted mobile document is active.'));
    }
    return adapter.readDocument(activeDocument);
  }, [activeDocument, adapter]);

  const library = <LibraryScreen state={sourceState} onOpen={openDocument} />;
  const content =
    tab === 'library' ? (
      library
    ) : tab === 'search' ? (
      <SearchScreen documents={documents} onOpen={openDocument} />
    ) : tab === 'sources' ? (
      <SourcesScreen
        savedConfig={savedConfig}
        sourceState={sourceState}
        onConnect={connect}
      />
    ) : (
      <SettingsScreen />
    );

  return (
    <div
      className={`mobile-shell mobile-shell--${platform}`}
      data-mobile-platform={platform}
      data-system-theme={theme === 'system' ? (systemDark ? 'dark' : 'light') : undefined}
    >
      <div className={`mobile-shell__content${tablet ? ' mobile-shell__content--tablet' : ''}`}>
        {activeTab && activeDocument ? (
          <>
            {tablet ? <div className="mobile-shell__library-pane">{library}</div> : null}
            <MobileReader
              tab={activeTab}
              mode={readerMode}
              sourceIdentity={activeDocument}
              loadDocument={readActiveDocument}
              onBack={closeReader}
              onModeChange={setReaderMode}
            />
          </>
        ) : (
          content
        )}
      </div>
      {!activeTab ? (
        <nav className="mobile-nav" aria-label="Primary">
          {NAVIGATION.map((item) => (
            <button
              key={item.id}
              type="button"
              className="mobile-nav__item"
              aria-current={tab === item.id ? 'page' : undefined}
              onClick={() => setTab(item.id)}
            >
              {item.label}
            </button>
          ))}
        </nav>
      ) : null}
    </div>
  );
}
