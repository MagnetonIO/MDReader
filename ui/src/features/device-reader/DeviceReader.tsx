import { useEffect, useMemo, useState, type ComponentProps } from 'react';
import type { AssetManifest } from '../../bindings';
import { DeviceAssetImage, DeviceAssetProvider, useDeviceAssetImages } from '../../mobile/DeviceAssetImage';
import { assetTarget } from '../../mobile/device-assets';
import { CanonicalReader } from '../reader/CanonicalReader';
import { deviceReaderBackend, DeviceReaderFailure, type DeviceReference, type DeviceReaderBackend } from './backend';
import { DeviceReadingControls } from './DeviceReadingControls';
import type { ReaderPosition } from '../reader-history/model';
import type { DeviceReadingStateBackend } from './reading-state-backend';

/** The normal inline reader, using the service's exact device owner and leases. */
export function DeviceReader({ reference, onClose, backend = deviceReaderBackend, readingStateBackend, ...history }: {
  reference: DeviceReference; onClose: () => void; backend?: DeviceReaderBackend; readingStateBackend?: DeviceReadingStateBackend;
} & Pick<ComponentProps<typeof CanonicalReader>, 'position' | 'onPosition' | 'onBack' | 'onForward' | 'canGoBack' | 'canGoForward' | 'onNavigateReference'>) {
  const [loaded, setLoaded] = useState<{ reference: DeviceReference; manifest: AssetManifest | null; error: string | null } | null>(null);
  const [visible, setVisible] = useState<{ reference: DeviceReference; position: ReaderPosition } | null>(null);
  const line = visible?.reference === reference ? visible.position.line : history.position?.revision === reference.revision
    ? history.position.line : reference.line ?? 1;
  const manifest = loaded?.reference === reference ? loaded.manifest : null;
  const error = loaded?.reference === reference ? loaded.error : null;
  const assetBackend = useMemo(() => backend.images(reference), [backend, reference]);
  const images = useDeviceAssetImages(manifest, assetBackend);
  const readerBackend = useMemo(() => ({
    readDocument: () => backend.read(reference),
    isReadRetryable: (cause: unknown) => cause instanceof DeviceReaderFailure && cause.kind === 'busy',
    resolveLink: backend.resolveLink ? (_origin: unknown, kind: 'relative' | 'wikilink', target: string) => backend.resolveLink!(reference, kind, target) : undefined,
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
      imageComponent={Image} onClose={onClose} {...history}
      onPosition={(position) => { setVisible({ reference, position }); history.onPosition?.(position); }}
      readingControls={(resume) => <DeviceReadingControls key={`${reference.documentId}:${reference.sourceId}:${reference.revision}`}
        reference={reference} line={line} onResume={resume} backend={readingStateBackend} />} />
  </DeviceAssetProvider>;
}
