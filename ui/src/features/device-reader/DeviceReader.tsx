import { useEffect, useMemo, useState, type ComponentProps } from 'react';
import type { AssetManifest } from '../../bindings';
import { DeviceAssetImage, DeviceAssetProvider, useDeviceAssetImages } from '../../mobile/DeviceAssetImage';
import { assetTarget } from '../../mobile/device-assets';
import { CanonicalReader } from '../reader/CanonicalReader';
import { deviceReaderBackend, type DeviceReference, type DeviceReaderBackend } from './backend';

/** The normal inline reader, using the service's exact device owner and leases. */
export function DeviceReader({ reference, onClose, backend = deviceReaderBackend, ...history }: {
  reference: DeviceReference; onClose: () => void; backend?: DeviceReaderBackend;
} & Pick<ComponentProps<typeof CanonicalReader>, 'position' | 'onPosition' | 'onBack' | 'onForward' | 'canGoBack' | 'canGoForward'>) {
  const [loaded, setLoaded] = useState<{ reference: DeviceReference; manifest: AssetManifest | null; error: string | null } | null>(null);
  const manifest = loaded?.reference === reference ? loaded.manifest : null;
  const error = loaded?.reference === reference ? loaded.error : null;
  const assetBackend = useMemo(() => backend.images(reference), [backend, reference]);
  const images = useDeviceAssetImages(manifest, assetBackend);
  const readerBackend = useMemo(() => ({
    readDocument: () => backend.read(reference),
  }), [backend, reference]);
  useEffect(() => {
    let active = true;
    setLoaded(null);
    void backend.manifest(reference).then((manifest) => {
      if (active) setLoaded({ reference, manifest, error: null });
    }).catch((cause: unknown) => {
      if (active) setLoaded({ reference, manifest: null, error: cause instanceof Error ? cause.message : 'The selected images are unavailable.' });
    });
    return () => { active = false; };
  }, [backend, reference]);
  const Image = useMemo(() => function SelectedDeviceImage({ src, alt }: { src?: string; alt?: string }) {
    const target = assetTarget(src ?? '');
    if (!target || !manifest?.assets.some((asset) => asset.target === target)) {
      return <span>{alt || 'Image'} — asset is not available in this copy.</span>;
    }
    return <DeviceAssetImage src={src} alt={alt} />;
  }, [manifest]);
  return <DeviceAssetProvider images={images}>
    {error ? <p role="alert">{error}</p> : null}
    <CanonicalReader reference={reference} backend={readerBackend} sourceKind="device"
      imageComponent={Image} onClose={onClose} {...history} />
  </DeviceAssetProvider>;
}
