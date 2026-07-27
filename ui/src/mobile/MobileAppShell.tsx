import { useEffect, useMemo, useState } from 'react';

import { listWorkspaceFiles } from '../platform/fs';
import {
  pickAndAddLibrarySource,
  watchLibrarySource,
} from '../platform/settings';
import { applyTheme, useSettingsStore } from '../stores/settings';
import { useSourcesStore } from '../stores/sources';
import { selectActiveTab, useTabsStore } from '../stores/tabs';
import { MobileReader } from './MobileReader';
import type { MobileSourceIdentity } from './offline-cache';
import {
  MOBILE_SOURCE_ALLOWLIST,
  saveEnterpriseServerConfig,
  validateEnterpriseServerConfig,
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

function LibraryScreen({ onOpen }: { onOpen: (path: string, name: string) => void }) {
  const files = useSourcesStore((state) => state.files);
  const root = useSourcesStore((state) => state.root);
  return (
    <section className="mobile-screen" aria-label="Library">
      <div className="mobile-screen__head">
        <span className="mobile-eyebrow">Library</span>
        <h1>{root ? root.split(/[\\/]/).filter(Boolean).at(-1) : 'Your documents'}</h1>
      </div>
      {files.length === 0 ? (
        <div className="mobile-state">
          <h2>No documents available</h2>
          <p>Add a device folder or connect an available source from Sources.</p>
        </div>
      ) : (
        <ol className="mobile-document-list">
          {files.map((file) => (
            <li key={file.path}>
              <button
                type="button"
                className="mobile-document"
                aria-label={file.name}
                onClick={() => onOpen(file.path, file.name)}
              >
                <span>{file.name}</span>
                <small>{Math.max(1, Math.ceil(file.size / 1024))} KiB</small>
              </button>
            </li>
          ))}
        </ol>
      )}
    </section>
  );
}

function SearchScreen({ onOpen }: { onOpen: (path: string, name: string) => void }) {
  const files = useSourcesStore((state) => state.files);
  const [query, setQuery] = useState('');
  const results = useMemo(() => {
    const needle = query.trim().toLocaleLowerCase();
    if (!needle) return [];
    return files.filter((file) => file.name.toLocaleLowerCase().includes(needle)).slice(0, 50);
  }, [files, query]);

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
            <li key={file.path}>
              <button
                type="button"
                className="mobile-document"
                aria-label={file.name}
                onClick={() => onOpen(file.path, file.name)}
              >
                {file.name}
              </button>
            </li>
          ))}
        </ol>
      )}
    </section>
  );
}

function SourcesScreen({
  onSourceIdentity,
}: {
  onSourceIdentity: (identity: MobileSourceIdentity) => void;
}) {
  const [message, setMessage] = useState<string | null>(null);
  const [serverError, setServerError] = useState<string | null>(null);
  const [serverUrl, setServerUrl] = useState('');
  const [sessionReference, setSessionReference] = useState('');

  const addDeviceFolder = async () => {
    setMessage(null);
    try {
      const picked = await pickAndAddLibrarySource();
      if (!picked.source) return;
      const grant = await watchLibrarySource(picked.source.id);
      const listing = await listWorkspaceFiles(grant.root);
      const sources = useSourcesStore.getState();
      sources.setRoot(grant.root);
      sources.setFiles(listing.files);
      sources.setFromIndex(listing.fromIndex);
      sources.setWatcher('watching');
      onSourceIdentity({ kind: 'device-folder', id: picked.source.id });
      setMessage(`${picked.source.name} is available in Library.`);
    } catch {
      setMessage('The device folder could not be added.');
    }
  };

  return (
    <section className="mobile-screen" aria-label="Sources">
      <div className="mobile-screen__head">
        <span className="mobile-eyebrow">Read-only membership</span>
        <h1>Sources</h1>
      </div>
      <div className="mobile-source-list">
        <article className="mobile-source">
          <h2>Device folders</h2>
          <p>Choose a folder the operating system makes available to MDReader.</p>
          <button type="button" className="mobile-action mobile-touch" onClick={addDeviceFolder}>
            Add folder
          </button>
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
                .then(() =>
                  setMessage('Server boundary saved. Authentication remains server-managed.'),
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
  const [sourceIdentity, setSourceIdentity] = useState<MobileSourceIdentity | null>(null);
  const [tablet] = useState(() => window.innerWidth >= 768);
  const theme = useSettingsStore((state) => state.theme);
  const activeTab = useTabsStore(selectActiveTab);
  const root = useSourcesStore((state) => state.root);

  useEffect(() => applyTheme(theme), [theme]);

  const openDocument = (path: string, name: string) => {
    useTabsStore.getState().openTab({ path, name }, { originRoot: root });
    setReaderMode('reader');
  };
  const closeReader = () => {
    if (activeTab) useTabsStore.getState().closeTab(activeTab.id);
    setReaderMode('reader');
  };

  const library = <LibraryScreen onOpen={openDocument} />;
  const content =
    tab === 'library' ? (
      library
    ) : tab === 'search' ? (
      <SearchScreen onOpen={openDocument} />
    ) : tab === 'sources' ? (
      <SourcesScreen onSourceIdentity={setSourceIdentity} />
    ) : (
      <SettingsScreen />
    );

  return (
    <div className={`mobile-shell mobile-shell--${platform}`} data-mobile-platform={platform}>
      <div className={`mobile-shell__content${tablet ? ' mobile-shell__content--tablet' : ''}`}>
        {activeTab ? (
          <>
            {tablet ? <div className="mobile-shell__library-pane">{library}</div> : null}
            <MobileReader
              tab={activeTab}
              mode={readerMode}
              sourceIdentity={sourceIdentity}
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
