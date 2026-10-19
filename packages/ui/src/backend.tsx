'use client';

import { createContext, useContext } from 'react';
import type { ReactNode } from 'react';

/**
 * A typed React seam for a host's native backend (the IPC surface behind the
 * reader). The package stays host-agnostic: the host owns the backend's shape
 * and implementation and passes its interface as `T`, so nothing here depends
 * on Tauri or on a host's generated bindings.
 *
 * `fallback` supplies the backend when no provider is mounted (tests, stories,
 * or modules rendered outside the host's root), so consumers never need a
 * null check.
 */
export function createBackendContext<T>(fallback: () => T) {
  const BackendContext = createContext<T | null>(null);

  function BackendProvider({ backend, children }: { backend: T; children: ReactNode }) {
    return <BackendContext.Provider value={backend}>{children}</BackendContext.Provider>;
  }

  function useBackend(): T {
    return useContext(BackendContext) ?? fallback();
  }

  return { BackendProvider, useBackend };
}
