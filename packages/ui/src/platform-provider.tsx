'use client';

import { createContext, useContext } from 'react';
import type { ReactNode } from 'react';

/**
 * Platform-specific capabilities the reader depends on. Implemented differently
 * by the desktop (Tauri) shell and the website.
 */
export interface Platform {
  /** Open a URL in the user's default browser / a new tab. */
  openExternal(url: string): void;
  /**
   * Resolve a relative asset/link URL (an image src or a relative markdown
   * link) against the current document's location, returning an absolute URL
   * the host can load.
   */
  resolveAssetUrl(url: string): string;
}

const PlatformContext = createContext<Platform | null>(null);

export function PlatformProvider({
  platform,
  children,
}: {
  platform: Platform;
  children: ReactNode;
}) {
  return <PlatformContext.Provider value={platform}>{children}</PlatformContext.Provider>;
}

/** Access the platform, throwing if no provider is present. */
export function usePlatform(): Platform {
  const platform = useContext(PlatformContext);
  if (platform === null) {
    throw new Error('usePlatform must be used within a PlatformProvider');
  }
  return platform;
}

/**
 * Access the platform if a provider is present, else null. The reader uses this
 * so it can render (with sensible fallbacks) outside a PlatformProvider — e.g.
 * in tests, stories, or a bare embed.
 */
export function useOptionalPlatform(): Platform | null {
  return useContext(PlatformContext);
}
