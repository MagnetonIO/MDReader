import { StrictMode } from 'react';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, test, vi } from 'vitest';
import type { AssetManifest, AssetReceipt, AssetReply, AssetRequest } from '../bindings';
import { assetTarget, DeviceAssetImages, uploadAsset, type AssetBackend } from './device-assets';
import { DeviceAssetImage, DeviceAssetProvider, useDeviceAssetImages } from './DeviceAssetImage';
import { DeviceAssetStorage } from './DeviceAssetStorage';
import { DeviceAssetsSection } from './DeviceAssetsSection';

const receipt = (id = 'asset-1'): AssetReceipt => ({ asset_id: id, document_id: 'document-1', document_revision: 'a'.repeat(64),
  target: `${id}.png`, content_hash: 'c'.repeat(64), mime: 'image/png', bytes: 3, width: 1, height: 1 });
const manifest = (assets: AssetReceipt[] = []): AssetManifest => ({ document_id: 'document-1', document_revision: 'a'.repeat(64),
  manifest: 'b'.repeat(64), assets, pending: [], targets: assets.map((item) => item.target), targets_truncated: false });
const complete: AssetReply = { operation: 'complete' };
function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>((done) => { resolve = done; }); return { promise, resolve }; }
function file(bytes: Uint8Array): File {
  return { size: bytes.length, slice: (start = 0, end = bytes.length) => ({ arrayBuffer: async () => bytes.slice(start, end).buffer }) } as File;
}
function imageBackend(calls: AssetRequest[], assets: AssetReceipt[], open?: (item: AssetReceipt) => Promise<AssetReply>): AssetBackend {
  return async (request) => {
    calls.push(request);
    if (request.operation === 'open') {
      const item = assets.find((asset) => asset.asset_id === request.asset_id)!;
      return open ? open(item) : { operation: 'opened', session_id: item.asset_id, receipt: item };
    }
    if (request.operation === 'chunk') return { operation: 'chunk', offset: request.offset, base64: 'AQID', complete: true };
    if (request.operation === 'close') return complete;
    throw new Error(`Unexpected ${request.operation}`);
  };
}
let sequence = 0;
const createUrl = vi.fn(() => `blob:asset-${++sequence}`);
const revokeUrl = vi.fn();
beforeEach(() => {
  sequence = 0; createUrl.mockClear(); revokeUrl.mockClear();
  vi.stubGlobal('URL', class extends URL { static override createObjectURL = createUrl; static override revokeObjectURL = revokeUrl; });
});
afterEach(() => { cleanup(); vi.unstubAllGlobals(); });

describe('selected native asset transport', () => {
  test('decodes relative targets once and rejects paths that could expand authority', () => {
    expect(assetTarget('./images/hello%20world.png')).toBe('images/hello world.png');
    for (const target of ['../x.png', '/x.png', 'https://x/a.png', 'x%252fy.png', 'a//b.png', 'a%00.png', 'a\\b.png', 'a.png?x', 'a.png#x']) expect(assetTarget(target)).toBeNull();
  });
  test('uploads bounded sequential chunks and replays an explicit pending copy from zero', async () => {
    const selected = manifest(); selected.targets = ['figure.png'];
    const data = new Uint8Array(64 * 1024 + 3).fill(7);
    selected.pending = [{ intent_id: 'pending', target: 'figure.png', bytes: data.length, received: 64 * 1024, state: 'reserved' }];
    const calls: AssetRequest[] = [];
    const backend: AssetBackend = async (request) => {
      calls.push(request);
      if (request.operation === 'write') return { operation: 'intent', intent_id: request.intent_id, received: Math.max(64 * 1024, request.offset + atob(request.base64).length) };
      if (request.operation === 'commit') return { operation: 'published', receipt: { ...receipt('pending'), target: 'figure.png', bytes: data.length }, manifest: selected.manifest };
      throw new Error('No new reservation is permitted during resume');
    };
    await uploadAsset(backend, selected, 'figure.png', file(data), 'pending');
    expect(calls.map((request) => request.operation)).toEqual(['write', 'write', 'commit']);
    const writes = calls.filter((request) => request.operation === 'write');
    expect(writes.map((request) => [request.offset, atob(request.base64).length])).toEqual([[0, 65536], [65536, 3]]);
  });
  test('selection cancellation after reservation preserves custody without writes or automatic discard', async () => {
    const selected = manifest(); selected.targets = ['figure.png'];
    const controller = new AbortController(); const calls: AssetRequest[] = [];
    const backend: AssetBackend = async (request) => { calls.push(request); controller.abort(); return { operation: 'intent', intent_id: 'pending', received: 0 }; };
    await expect(uploadAsset(backend, selected, 'figure.png', file(new Uint8Array([1])), undefined, controller.signal)).rejects.toThrow('pending copy remains');
    expect(calls.map((request) => request.operation)).toEqual(['begin']);
  });
  test('limits concurrent snapshots and decoded URLs to two, closing every native session', async () => {
    const assets: [AssetReceipt, AssetReceipt, AssetReceipt] = [receipt('one'), receipt('two'), receipt('three')]; const calls: AssetRequest[] = [];
    const gates = [deferred<AssetReply>(), deferred<AssetReply>(), deferred<AssetReply>()] as const;
    const images = new DeviceAssetImages(manifest(assets), imageBackend(calls, assets, (item) => gates[assets.indexOf(item)]!.promise));
    assets.forEach((item) => images.retain(item.asset_id, item.asset_id));
    const loads = assets.map((item) => images.load(item));
    expect(calls.filter((request) => request.operation === 'open')).toHaveLength(2);
    gates[0].resolve({ operation: 'opened', session_id: 'one', receipt: assets[0] });
    await waitFor(() => expect(calls.filter((request) => request.operation === 'open')).toHaveLength(3));
    gates[1].resolve({ operation: 'opened', session_id: 'two', receipt: assets[1] });
    gates[2].resolve({ operation: 'opened', session_id: 'three', receipt: assets[2] });
    await Promise.all(loads);
    expect(createUrl).toHaveBeenCalledTimes(3); expect(revokeUrl).toHaveBeenCalledTimes(1);
    expect(assets.filter((item) => images.url(item.asset_id))).toHaveLength(2);
    expect(calls.filter((request) => request.operation === 'close')).toHaveLength(3);
    images.dispose(); expect(revokeUrl).toHaveBeenCalledTimes(3);
  });
  test('a late snapshot after unmount closes without reading bytes or creating a Blob URL', async () => {
    const item = receipt(); const calls: AssetRequest[] = []; const gate = deferred<AssetReply>();
    const images = new DeviceAssetImages(manifest([item]), imageBackend(calls, [item], () => gate.promise));
    images.retain(item.asset_id, 'view'); const loading = images.load(item);
    const rejected = expect(loading).rejects.toThrow('selection or visibility changed');
    images.dispose(); gate.resolve({ operation: 'opened', session_id: 'late', receipt: item });
    await rejected;
    expect(calls.map((request) => request.operation)).toEqual(['open', 'close']);
    expect(createUrl).not.toHaveBeenCalled();
  });
});

describe('device asset presentation', () => {
  test('loads only near-visible selected images and revokes their URL offscreen', async () => {
    let visibility!: (entries: Partial<IntersectionObserverEntry>[]) => void;
    const disconnect = vi.fn();
    vi.stubGlobal('IntersectionObserver', class { constructor(callback: typeof visibility) { visibility = callback; } observe() {} disconnect = disconnect; });
    const item = receipt(); const calls: AssetRequest[] = [];
    const images = new DeviceAssetImages(manifest([item]), imageBackend(calls, [item]));
    const view = render(<DeviceAssetProvider images={images}><DeviceAssetImage src={item.target} alt="Diagram" /></DeviceAssetProvider>);
    expect(calls).toEqual([]); expect(screen.queryByRole('img')).toBeNull();
    act(() => visibility([{ isIntersecting: true }]));
    expect((await screen.findByRole('img', { name: 'Diagram' })).getAttribute('src')).toBe('blob:asset-1');
    expect(calls[0]).toEqual({ operation: 'open', document_id: item.document_id, revision: item.document_revision, asset_id: item.asset_id, manifest: images.manifest.manifest });
    act(() => visibility([{ isIntersecting: false }]));
    await waitFor(() => expect(revokeUrl).toHaveBeenCalledWith('blob:asset-1'));
    expect(screen.queryByRole('img')).toBeNull(); view.unmount(); images.dispose(); expect(disconnect).toHaveBeenCalled();
  });
  test('StrictMode owns a fresh manager and revision changes fence and revoke old bytes', async () => {
    vi.stubGlobal('IntersectionObserver', undefined);
    const item = receipt(); const selected = manifest([item]); const calls: AssetRequest[] = [];
    const backend = imageBackend(calls, [item]);
    function View({ current }: { current: AssetManifest }) {
      const images = useDeviceAssetImages(current, backend);
      return <DeviceAssetProvider images={images}><DeviceAssetImage src={item.target} alt="Strict diagram" /></DeviceAssetProvider>;
    }
    const view = render(<StrictMode><View current={selected} /></StrictMode>);
    fireEvent.click(await screen.findByRole('button', { name: 'Load selected offline image' }));
    expect((await screen.findByRole('img', { name: 'Strict diagram' })).getAttribute('src')).toBe('blob:asset-1');
    const next = { ...manifest(), document_revision: 'd'.repeat(64), manifest: 'e'.repeat(64) };
    view.rerender(<StrictMode><View current={next} /></StrictMode>);
    expect(screen.queryByRole('img')).toBeNull();
    expect(revokeUrl).toHaveBeenCalledWith('blob:asset-1');
    expect(calls.filter((request) => request.operation === 'open')).toHaveLength(1);
    view.unmount();
  });
  test('unselected local and HTTPS images do not trigger native or remote loads without explicit selection', () => {
    const calls: AssetRequest[] = []; const images = new DeviceAssetImages(manifest(), imageBackend(calls, []));
    render(<DeviceAssetProvider images={images}><DeviceAssetImage src="private.png" /><DeviceAssetImage src="https://example.test/image.png" alt="Remote" /></DeviceAssetProvider>);
    expect(calls).toEqual([]); expect(screen.queryByRole('img')).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'Load remote image' }));
    expect(screen.getByRole('img', { name: 'Remote' }).getAttribute('src')).toBe('https://example.test/image.png'); images.dispose();
  });
  test('the explicit file picker refreshes uncertain publication without discarding custody', async () => {
    const selected = manifest(); selected.targets = ['figure.png']; const calls: AssetRequest[] = []; const refresh = vi.fn();
    const backend: AssetBackend = async (request) => {
      calls.push(request);
      if (request.operation === 'begin') return { operation: 'intent', intent_id: 'pending', received: 0 };
      if (request.operation === 'write') return { operation: 'intent', intent_id: 'pending', received: 3 };
      if (request.operation === 'commit') throw new Error('Publication reply unavailable; reload to reconcile');
      throw new Error('No automatic discard is authorized');
    };
    render(<DeviceAssetsSection documentId={selected.document_id} revision={selected.document_revision} manifest={selected} onRefresh={refresh} backend={backend} />);
    fireEvent.change(screen.getByLabelText('Image target in this Markdown'), { target: { value: 'figure.png' } });
    fireEvent.change(screen.getByLabelText('Choose the individual image'), { target: { files: [file(new Uint8Array([1, 2, 3]))] } });
    expect((await screen.findByRole('alert')).textContent).toContain('Publication reply unavailable');
    expect(refresh).toHaveBeenCalledTimes(1);
    expect(calls.map((request) => request.operation)).toEqual(['begin', 'write', 'commit']);
  });
  test('explicit metadata-only cleanup reconciles a lost reply and reports recovered quota', async () => {
    const calls: AssetRequest[] = []; const changed = vi.fn(); let forgotten = false;
    const item = { asset_id: 'retired', document_id: 'revoked-document', document_revision: 'a'.repeat(64), target: 'old.png', bytes: 123, received: 123, state: 'retired' };
    const backend: AssetBackend = async (request) => {
      calls.push(request);
      if (request.operation === 'forget') { forgotten = true; throw new Error('Reply unavailable; reconcile storage'); }
      if (request.operation === 'inventory') return { operation: 'inventory', value: { items: forgotten ? [] : [item], reserved_bytes: forgotten ? 0 : 123, max_bytes: 33554432, max_items: 512 } };
      throw new Error('Cleanup must not read a revoked document or image');
    };
    render(<DeviceAssetStorage backend={backend} onChanged={changed} />);
    expect(calls).toEqual([]); fireEvent.click(screen.getByRole('button', { name: 'Review offline image storage' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Forget offline copy of old.png' }));
    expect((await screen.findByRole('status')).textContent).toContain('Recovered 123 bytes');
    expect(screen.getByRole('alert').textContent).toContain('Reply unavailable'); expect(changed).toHaveBeenCalledTimes(1);
    expect(calls.map((request) => request.operation)).toEqual(['inventory', 'forget', 'inventory']);
  });
});
