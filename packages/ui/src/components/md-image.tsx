'use client';

import { useReaderContext } from '../reader-context.js';

export interface MdImageProps {
  src?: string;
  alt?: string;
  title?: string;
}

/** Absolute URL (including data: and protocol-relative). */
function isAbsolute(src: string): boolean {
  return /^(?:[a-z][a-z0-9+.-]*:|\/\/)/i.test(src);
}

/**
 * Renders a markdown image, resolving relative sources against the document
 * location via the platform's `resolveAssetUrl`. Absolute and data: URLs pass
 * through unchanged.
 *
 * `draggable={false}`: an `<img>` is natively draggable, and the resolved source
 * of a local asset is an app-internal URL that means nothing dropped elsewhere.
 * (A selection that merely crosses an image was never affected — measured.)
 */
export function MdImage({ src = '', alt = '', title }: MdImageProps) {
  const { platform } = useReaderContext();
  const resolved = platform && src && !isAbsolute(src) ? platform.resolveAssetUrl(src) : src;

  return (
    <img
      src={resolved}
      alt={alt}
      title={title}
      loading="lazy"
      draggable={false}
      style={{ maxWidth: '100%', height: 'auto', borderRadius: 'var(--radius-sm)' }}
    />
  );
}
