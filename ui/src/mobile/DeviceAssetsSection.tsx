import { useEffect, useRef, useState } from 'react';
import type { AssetManifest } from '../bindings';
import { nativeAssetBackend, uploadAsset, type AssetBackend } from './device-assets';

export function DeviceAssetsSection({ documentId, revision, manifest, onRefresh, backend = nativeAssetBackend }: {
  documentId: string;
  revision: string;
  manifest: AssetManifest | null;
  onRefresh: () => void;
  backend?: AssetBackend;
}) {
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [target, setTarget] = useState('');
  const identity = `${documentId}:${revision}`;
  const current = useRef(identity); current.current = identity;
  const pendingOperation = useRef<AbortController | null>(null);
  const selected = manifest?.document_id === documentId && manifest.document_revision === revision ? manifest : null;
  useEffect(() => {
    current.current = identity; setTarget(''); setError(null); setBusy(false);
    return () => {
      if (current.current === identity) current.current = '';
      pendingOperation.current?.abort(); pendingOperation.current = null;
    };
  }, [identity]);
  async function operation(run: (signal: AbortSignal) => Promise<unknown>) {
    if (pendingOperation.current) return;
    const controller = new AbortController(); pendingOperation.current = controller;
    setBusy(true); setError(null);
    const started = identity;
    try { await run(controller.signal); }
    catch (cause) {
      if (started === current.current) setError(cause instanceof Error ? cause.message : 'The image operation failed.');
    } finally {
      if (pendingOperation.current === controller) pendingOperation.current = null;
      if (started === current.current) { setBusy(false); onRefresh(); }
    }
  }
  return <details className="mobile-state">
    <summary>Offline images</summary>
    <p>Select each PNG or JPEG for this exact Markdown revision. No folder access is inferred.
      Up to 4 MiB and 4 million pixels per image; 8 assets and 16 MiB per document,
      32 MiB for the device library including incomplete copies.
      SVG, animated PNG and raw HTML image grants are unsupported.</p>
    {!selected ? <p>Reading selected asset metadata…</p> : <>
      {selected.targets_truncated ? <p>Only the first 64 native Markdown image targets are offered for selection.</p> : null}
      <label>Image target in this Markdown
        <select aria-label="Image target in this Markdown" disabled={busy} value={target}
          onChange={(event) => setTarget(event.currentTarget.value)}>
          <option value="">Choose an image target</option>
          {selected.targets.map((value) => <option key={value} value={value}>{value}</option>)}
        </select>
      </label>
      <label>Choose the individual image
        <input aria-label="Choose the individual image" type="file" accept="image/png,image/jpeg" disabled={busy || !target}
          onChange={(event) => {
            const file = event.currentTarget.files?.[0]; event.currentTarget.value = '';
            if (file) void operation((signal) => uploadAsset(backend, selected, target, file, undefined, signal));
          }} />
      </label>
      {selected.assets.map((asset) => <p key={asset.asset_id}>
        {asset.target} · {asset.mime} · {asset.width}×{asset.height}
        <button type="button" disabled={busy} onClick={() => void operation(() => backend({
          operation: 'remove', document_id: documentId, revision,
          asset_id: asset.asset_id, expected_manifest: selected.manifest,
        }))}>Remove selected {asset.target}</button>
      </p>)}
      {selected.pending.map((pending) => <div key={pending.intent_id}>
        <p>{pending.target} · {pending.state} · {pending.received}/{pending.bytes} bytes.
          Storage remains reserved until publication or discard.</p>
        {pending.state === 'reserved' && pending.received === pending.bytes ?
          <button type="button" disabled={busy} onClick={() => void operation(() => backend({ operation: 'commit', intent_id: pending.intent_id }))}>
            Finish selection {pending.target}
          </button> : pending.state === 'reserved' ? <label>Resume {pending.target}
            <input aria-label={`Resume ${pending.target}`} type="file" accept="image/png,image/jpeg" disabled={busy}
              onChange={(event) => {
                const file = event.currentTarget.files?.[0]; event.currentTarget.value = '';
                if (file) void operation((signal) => uploadAsset(backend, selected, pending.target, file, pending.intent_id, signal));
              }} />
          </label> : null}
        <button type="button" disabled={busy} onClick={() => void operation(() => backend({ operation: 'discard', intent_id: pending.intent_id }))}>
          Discard incomplete or replaced {pending.target}
        </button>
      </div>)}
    </>}
    {busy ? <p role="status">Copying selected image…</p> : null}
    {error ? <p role="alert">{error}</p> : null}
  </details>;
}
