import { createContext, useContext, useEffect, useId, useRef, useState, useSyncExternalStore } from 'react';
import type { PropsWithChildren } from 'react';
import type { LocalImageBytes } from './contracts';
import { LocalImages, relativeImageTarget, type LocalImageController } from './local-images';

const Images = createContext<LocalImageController | null>(null);
const inactive = () => () => {};

export function useLocalImages<Reference>(reference: Reference, read: ((reference: Reference, target: string) => Promise<LocalImageBytes>) | undefined, enabled: boolean) {
  const [owned, setOwned] = useState<{ reference: Reference; images: LocalImages<Reference> } | null>(null);
  useEffect(() => {
    if (!enabled || !read) { setOwned(null); return; }
    const images = new LocalImages(reference, read);
    setOwned({ reference, images });
    return () => images.dispose();
  }, [reference, read, enabled]);
  return enabled && owned?.reference === reference ? owned.images : null;
}

export function LocalImageProvider({ images, children }: PropsWithChildren<{ images: LocalImageController | null }>) {
  return <Images.Provider value={images}>{children}</Images.Provider>;
}

export function LocalSourceImage({ src, alt = '' }: { src?: string; alt?: string }) {
  const images = useContext(Images);
  const target = relativeImageTarget(src);
  const current = useRef({ images, target }); current.current = { images, target };
  const host = useRef<HTMLSpanElement>(null);
  const lease = useId();
  const [visible, setVisible] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const url = useSyncExternalStore(images?.subscribe ?? inactive,
    () => target && images ? images.url(target) : null, () => null);
  useEffect(() => {
    setVisible(false); setError(null);
    if (!host.current || !target || !images || typeof IntersectionObserver === 'undefined') return;
    const observer = new IntersectionObserver(([entry]) => setVisible(Boolean(entry?.isIntersecting)), { rootMargin: '200px' });
    observer.observe(host.current);
    return () => observer.disconnect();
  }, [images, target]);
  useEffect(() => {
    if (!images || !target || !visible) return;
    let active = true;
    images.retain(target, lease);
    void images.load(target).catch((cause: unknown) => {
      if (active) setError(cause instanceof Error ? cause.message : 'This image is unavailable.');
    });
    return () => { active = false; images.release(target, lease); };
  }, [images, target, visible, lease]);
  const load = () => {
    if (!images || !target) return;
    setError(null);
    if (!visible) { setVisible(true); return; }
    void images.load(target).catch((cause: unknown) => {
      if (current.current.images === images && current.current.target === target) {
        setError(cause instanceof Error ? cause.message : 'This image is unavailable.');
      }
    });
  };
  return <span ref={host} className="local-source-image">
    {target && images && visible && url
      ? <img src={url} alt={alt} loading="lazy" draggable={false} style={{ maxWidth: '100%', height: 'auto' }} />
      : <span>{alt || 'Image'} — {target && images ? <button type="button" onClick={load}>Load local image</button> : 'local image unavailable'}
        {error ? <span role="alert">{error}</span> : null}</span>}
  </span>;
}
