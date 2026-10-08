import { createContext, useContext, useEffect, useId, useRef, useState, useSyncExternalStore } from 'react';
import type { PropsWithChildren } from 'react';
import { assetTarget, DeviceAssetImages } from './device-assets';
const ImageContext = createContext<DeviceAssetImages | null>(null);
const inactiveSubscription = () => () => {};
/** Construct inside the effect so StrictMode setup/cleanup gets a fresh owner.
 * The identity check removes old URLs during render, before cleanup effects. */
export function useDeviceAssetImages(manifest: DeviceAssetImages['manifest'] | null, backend?: ConstructorParameters<typeof DeviceAssetImages>[1]) {
  const [owned, setOwned] = useState<{ manifest: DeviceAssetImages['manifest']; images: DeviceAssetImages } | null>(null);
  useEffect(() => {
    if (!manifest) { setOwned(null); return; }
    const images = new DeviceAssetImages(manifest, backend);
    setOwned({ manifest, images });
    return () => images.dispose();
  }, [manifest, backend]);
  return owned?.manifest === manifest ? owned.images : null;
}
export function DeviceAssetProvider({ images, children }: PropsWithChildren<{ images: DeviceAssetImages | null }>) {
  return <ImageContext.Provider value={images}>{children}</ImageContext.Provider>;
}
/** Only selected relative bytes enter offline Blob URLs. HTTPS has its own
 * explicit consent and never becomes an offline attachment implicitly. */
export function DeviceAssetImage({ src, alt = '' }: { src?: string; alt?: string }) {
  const images = useContext(ImageContext);
  const target = assetTarget(src ?? '');
  const receipt = target ? images?.manifest.assets.find((asset) => asset.target === target) : null;
  const identity = `${images?.manifest.manifest ?? ''}:${receipt?.asset_id ?? src ?? ''}`;
  const current = useRef(identity); current.current = identity;
  const host = useRef<HTMLSpanElement>(null);
  const lease = useId();
  const [intersection, setIntersection] = useState({ identity: '', visible: false });
  const visible = intersection.identity === identity && intersection.visible;
  const [failure, setFailure] = useState({ identity: '', message: '' });
  const url = useSyncExternalStore(images?.subscribe ?? inactiveSubscription,
    () => receipt && images ? images.url(receipt.asset_id) : null, () => null);
  useEffect(() => {
    const element = host.current;
    if (!element || !receipt || typeof IntersectionObserver === 'undefined') return;
    const observer = new IntersectionObserver(([entry]) => setIntersection({ identity, visible: Boolean(entry?.isIntersecting) }), { rootMargin: '200px' });
    observer.observe(element); return () => observer.disconnect();
  }, [images, receipt, identity]);
  useEffect(() => {
    if (!images || !receipt || !visible) return;
    let active = true; images.retain(receipt.asset_id, lease); setFailure({ identity, message: '' });
    void images.load(receipt).catch((cause: unknown) => {
      if (active) setFailure({ identity, message: cause instanceof Error ? cause.message : 'The selected image is unavailable.' });
    });
    return () => { active = false; images.release(receipt.asset_id, lease); };
  }, [images, receipt, visible, lease, identity]);
  useEffect(() => () => { if (current.current === identity) current.current = ''; }, [identity]);
  const [approvedUrl, setApprovedUrl] = useState<string | null>(null);
  let remote = false;
  try { const parsed = new URL(src ?? ''); remote = parsed.protocol === 'https:' && !parsed.username && !parsed.password; } catch { /* Unselected relative targets grant no filesystem authority. */ }
  const load = () => {
    if (!images || !receipt) return;
    setIntersection({ identity, visible: true });
    if (!visible) return;
    void images.load(receipt).catch((cause: unknown) => {
      if (current.current === identity) setFailure({ identity, message: cause instanceof Error ? cause.message : 'The selected image is unavailable.' });
    });
  };
  return <span ref={host} className="mobile-offline-image">
    {receipt && visible && url ? <img src={url} alt={alt} width={receipt.width} height={receipt.height} loading="lazy" draggable={false} style={{ maxWidth: '100%', height: 'auto' }} />
      : receipt && images ? <span className="mobile-state">{alt || 'Selected offline image'} — <button type="button" onClick={load}>Load selected offline image</button>{failure.identity === identity && failure.message ? <span role="alert">{failure.message}</span> : null}</span>
        : remote && approvedUrl === src ? <img src={src} alt={alt} loading="lazy" draggable={false} style={{ maxWidth: '100%', height: 'auto' }} />
          : <span className="mobile-state">{alt || 'Image'} — {remote ? <button type="button" onClick={() => setApprovedUrl(src ?? null)}>Load remote image</button> : 'asset is not selected; choose it in Offline images'}</span>}
  </span>;
}
