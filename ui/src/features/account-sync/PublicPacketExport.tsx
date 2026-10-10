import { useState } from 'react';
import { downloadPublicPacket, parsePublicPacket, type PublicPairPacket } from './public-packet';
import { PublicPacketIdentity } from './PublicPacketIdentity';
export function PublicPacketExport({ text, kind, compact = false }: { text: string; kind: string; compact?: boolean }) {
  const [error, setError] = useState<string | null>(null);
  let packet: PublicPairPacket | null = null;
  try { packet = parsePublicPacket(text); } catch { /* Native output retained below for diagnostics; unverified text is never downloaded. */ }
  return <section className={`account-public-transfer${compact ? " account-public-transfer--compact" : ""}`} aria-label="Device file to share">
    {!compact ? <h4>{kind === 'request' ? 'Your device request is ready' : 'Your device approval is ready'}</h4> : null}
    {!compact ? <p>{kind === 'request' ? 'Save this public file and open it on your connected device under Add another device.' : 'Save this public file and return it to the requesting device. Compare all identities before it accepts approval.'}</p> : null}
    <button type="button" disabled={!packet} onClick={() => { try { downloadPublicPacket(text); setError(null); } catch { setError('The public device file could not be saved. Use Advanced to copy the public JSON.'); } }}>Save {kind === 'request' ? 'device request' : 'device approval'} file</button>
    {error ? <p role="alert">{error}</p> : null}
    {packet ? <PublicPacketIdentity packet={packet} /> : <p>The device file format could not be verified. Refresh status before sharing it.</p>}
    <details className="settings-account-advanced"><summary>Advanced · public JSON</summary><label>Public {kind} packet<textarea readOnly value={text} aria-label="Public device packet" /></label></details>
  </section>;
}
