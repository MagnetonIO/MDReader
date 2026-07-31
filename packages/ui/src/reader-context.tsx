'use client';

import { createContext, useContext } from 'react';

import type { Platform } from './platform-provider.js';

/** A navigation event emitted by an in-document link. */
export type NavigateEvent =
  | { kind: 'wikilink'; target: string; heading: string | null }
  | { kind: 'anchor'; slug: string }
  | { kind: 'relative'; href: string };

/** Shared reader state made available to the substituted components. */
export interface ReaderContextValue {
  /** Invoked when an in-document link is activated. */
  onNavigate?: (event: NavigateEvent) => void;
  /** Platform capabilities, or null when rendered without a PlatformProvider. */
  platform: Platform | null;
  /** The currently active heading slug (scroll-spy), for anchor emphasis. */
  activeSlug?: string;
  /** Optional resolver marking whether a wikilink target exists. */
  resolveWikiLink?: (target: string) => boolean;
  /**
   * Register a mounted heading element with the reader so it can drive
   * scroll-spy from the actual rendered DOM. Returns an unregister callback.
   * Absent when the reader is rendered without a provider (bare components).
   */
  registerHeading?: (slug: string, element: HTMLElement) => () => void;
}

const ReaderContext = createContext<ReaderContextValue>({ platform: null });

export const ReaderProvider = ReaderContext.Provider;

export function useReaderContext(): ReaderContextValue {
  return useContext(ReaderContext);
}
