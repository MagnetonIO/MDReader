import { useId, useState } from 'react';
import type { CanonicalReaderReference } from '../reader/contracts';
import { parseReadingLink } from './reading-link';
import './document-transfer.css';

export function ReadingLinkOpen({ onOpen }: { onOpen(reference: CanonicalReaderReference): void | Promise<void> }) {
  const id = useId();
  const [value, setValue] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  async function open() {
    setError(null); setBusy(true);
    try { await onOpen(parseReadingLink(value.trim())); setValue(''); }
    catch (cause) { setError(cause instanceof Error ? cause.message : 'This exact reading link is unavailable.'); }
    finally { setBusy(false); }
  }
  return <details className="reading-link-open">
    <summary>Open reading link</summary>
    <p>Paste an exact receipt from this library. Missing or revoked sources stay unavailable; this does not connect a source or fetch a newer revision.</p>
    <label htmlFor={id}>MDReader reading link</label>
    <input id={id} value={value} maxLength={16384} onChange={(event) => setValue(event.target.value)} />
    <button type="button" disabled={!value.trim() || busy} onClick={() => void open()}>Open exact revision</button>
    {error ? <p role="alert">{error}</p> : null}
  </details>;
}
