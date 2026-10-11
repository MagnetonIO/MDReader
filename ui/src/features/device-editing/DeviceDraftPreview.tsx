import { useEffect, useMemo, useState } from 'react';
import type { AssetManifest } from '../../bindings';
import { DeviceAssetImage, DeviceAssetProvider, useDeviceAssetImages } from '../../mobile/DeviceAssetImage';
import { assetTarget } from '../../mobile/device-assets';
import type { DeviceReaderBackend, DeviceReference } from '../device-reader/backend';
import { DraftPreview } from '../editor-controls/DraftPreview';

/** Draft images retain the saved copy's exact manifest and native asset leases. */
export function DeviceDraftPreview({ identity, markdown, reference, backend }: {
  identity: string; markdown: string; reference: DeviceReference; backend: DeviceReaderBackend;
}) {
  const [loaded, setLoaded] = useState<{ reference: DeviceReference; manifest: AssetManifest | null } | null>(null);
  const manifest = loaded?.reference === reference ? loaded.manifest : null;
  const assetBackend = useMemo(() => backend.images(reference), [backend, reference]);
  const images = useDeviceAssetImages(manifest, assetBackend);
  useEffect(() => {
    let active = true;
    void backend.manifest(reference).then(manifest => { if (active) setLoaded({ reference, manifest }); }).catch(() => {
      if (active) setLoaded({ reference, manifest: null });
    });
    return () => { active = false; };
  }, [reference, backend]);
  const Image = useMemo(() => function SavedCopyImage({ src, alt }: { src?: string; alt?: string }) {
    const target = assetTarget(src ?? '');
    return target && manifest?.assets.some(asset => asset.target === target)
      ? <DeviceAssetImage src={src} alt={alt} /> : <span>{alt || 'Image'} — asset is not available in this copy.</span>;
  }, [manifest]);
  return <DeviceAssetProvider images={images}><DraftPreview identity={identity} markdown={markdown} Image={Image} /></DeviceAssetProvider>;
}
