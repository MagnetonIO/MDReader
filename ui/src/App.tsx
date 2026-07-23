import { ReaderPlaceholder } from '@mdreader/ui';
import { useState } from 'react';

import { formatPing, pingBackend } from './platform/tauri';
import './App.css';

export default function App() {
  const [result, setResult] = useState<string>('');
  const [error, setError] = useState<string>('');

  async function handlePing() {
    setError('');
    try {
      setResult(formatPing(await pingBackend('hello from the webview')));
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }

  return (
    <main className="app">
      <h1>MDReader</h1>
      <ReaderPlaceholder />
      <section className="ipc">
        <button type="button" onClick={handlePing}>
          Ping backend
        </button>
        {result ? <p data-testid="ping-result">{result}</p> : null}
        {error ? <p data-testid="ping-error">IPC unavailable: {error}</p> : null}
      </section>
    </main>
  );
}
