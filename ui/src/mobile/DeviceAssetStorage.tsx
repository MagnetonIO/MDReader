import { useEffect, useRef, useState } from 'react';
import type { AssetReply } from '../bindings';
import { nativeAssetBackend, type AssetBackend } from './device-assets';
type Inventory = Extract<AssetReply, { operation: 'inventory' }>['value'];
/** Metadata-only cleanup is reachable even when a source has been revoked or
 * its original document is unavailable. It never grants image read authority. */
export function DeviceAssetStorage({ onChanged, backend = nativeAssetBackend }: { onChanged: () => void; backend?: AssetBackend }) {
  const [inventory, setInventory] = useState<Inventory | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const mounted = useRef(true);
  const running = useRef(false);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  async function readInventory() {
    const reply = await backend({ operation: 'inventory' });
    if (reply.operation !== 'inventory') throw new Error('Asset storage metadata could not be verified.');
    return reply.value;
  }
  async function review(assetId?: string) {
    if (running.current) return;
    running.current = true; setBusy(true); setError(null); setNotice(null);
    const before = inventory?.reserved_bytes;
    try {
      if (assetId) {
        const reply = await backend({ operation: 'forget', asset_id: assetId });
        if (reply.operation !== 'complete') throw new Error('The cleanup reply could not be verified. Reload storage to reconcile.');
      }
    } catch (cause) {
      if (mounted.current) setError(cause instanceof Error ? cause.message : 'The asset cleanup reply is unavailable. Reload storage to reconcile.');
    }
    try {
      const value = await readInventory();
      if (mounted.current) {
        setInventory(value);
        if (assetId && before !== undefined) setNotice(`Recovered ${Math.max(0, before - value.reserved_bytes)} bytes of asset quota.`);
        if (assetId) onChanged();
      }
    } catch (cause) {
      if (mounted.current) setError(cause instanceof Error ? cause.message : 'Asset storage metadata is unavailable.');
    } finally {
      running.current = false; if (mounted.current) setBusy(false);
    }
  }
  return <article className="mobile-source">
    <h2>Offline image storage</h2>
    <p>Selected, incomplete and replaced images all use the device asset quota.
      Explicitly forget a local copy to recover storage, including copies whose document is unavailable.
      Original selected files and Markdown are preserved.</p>
    <button type="button" className="mobile-touch" disabled={busy} onClick={() => void review()}>
      Review offline image storage
    </button>
    {inventory ? <>
      <p>{inventory.reserved_bytes} / {inventory.max_bytes} bytes reserved · {inventory.items.length} / {inventory.max_items} copies</p>
      {inventory.items.map((item) => <div key={item.asset_id}>
        <p>{item.target} · {item.state} · {item.received}/{item.bytes} bytes
          · Document {item.document_id.slice(0, 8)} · Revision {item.document_revision.slice(0, 8)}</p>
        <button type="button" className="mobile-touch" disabled={busy} onClick={() => void review(item.asset_id)}>
          Forget offline copy of {item.target}
        </button>
      </div>)}
    </> : null}
    {notice ? <p role="status">{notice}</p> : null}
    {error ? <p role="alert">{error}</p> : null}
  </article>;
}
