import { useEffect, useState } from 'react';

import { readerBackend } from '../platform/backend.js';
import { isTauri } from './persistence.js';

export interface AvailableUpdate {
  version: string;
  currentVersion: string;
  notes: string | null;
}

export interface UpdateAdapter {
  capability: () => Promise<boolean>;
  check: () => Promise<AvailableUpdate | null>;
  install: (expectedVersion: string) => Promise<void>;
}

function errorMessage(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}

const defaultAdapter: UpdateAdapter = {
  capability: async () => isTauri() && readerBackend().updates.updaterAvailable(),
  check: async () => {
    const result = await readerBackend().updates.checkForUpdate();
    if (result.status === 'error') throw new Error(result.error);
    return result.data;
  },
  install: async (expectedVersion) => {
    const result = await readerBackend().updates.installUpdate(expectedVersion);
    if (result.status === 'error') throw new Error(result.error);
  },
};

export function UpdateControl({ adapter = defaultAdapter }: { adapter?: UpdateAdapter }) {
  const [available, setAvailable] = useState(false);
  const [update, setUpdate] = useState<AvailableUpdate | null>(null);
  const [status, setStatus] = useState<
    'idle' | 'checking' | 'ready' | 'installing' | 'installed' | 'error'
  >('idle');
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let current = true;
    void adapter
      .capability()
      .then((value) => {
        if (current) setAvailable(value);
      })
      .catch(() => {
        if (current) setAvailable(false);
      });
    return () => {
      current = false;
    };
  }, [adapter]);

  if (!available) return null;

  const check = async () => {
    setStatus('checking');
    setError(null);
    try {
      const next = await adapter.check();
      setUpdate(next);
      setStatus(next ? 'ready' : 'idle');
    } catch (cause) {
      setError(`Could not check for updates — ${errorMessage(cause)}`);
      setStatus('error');
    }
  };

  const install = async () => {
    if (!update) return;
    setStatus('installing');
    setError(null);
    try {
      await adapter.install(update.version);
      setStatus('installed');
    } catch (cause) {
      setError(`Could not install the update — ${errorMessage(cause)}`);
      setStatus('error');
    }
  };

  return (
    <div className="update-control">
      {status === 'ready' && update ? (
        <>
          <span>MDReader {update.version} is available</span>
          <button
            className="icon-btn"
            type="button"
            aria-label={`Install update ${update.version}`}
            onClick={() => void install()}
          >
            Install
          </button>
        </>
      ) : status === 'installed' ? (
        <span role="status">Update installed. Restart MDReader to finish.</span>
      ) : (
        <button
          className="icon-btn"
          type="button"
          onClick={() => void check()}
          disabled={status === 'checking' || status === 'installing'}
          aria-label="Check for updates"
        >
          {status === 'checking' ? 'Checking…' : 'Updates'}
        </button>
      )}
      {error ? <span role="alert">{error}</span> : null}
    </div>
  );
}
