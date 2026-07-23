/** Minimal transient-toast store + presenter. Used for the "Updated" live-reload
 * nudge and the "search arrives in P4" Cmd+K placeholder. */

import { useEffect } from 'react';
import { create } from 'zustand';

interface Toast {
  id: number;
  message: string;
  ttl: number;
}

interface ToastState {
  toasts: Toast[];
  push: (message: string, ttl?: number) => void;
  dismiss: (id: number) => void;
}

let seq = 0;

export const useToastStore = create<ToastState>((set) => ({
  toasts: [],
  push: (message, ttl = 2600) =>
    set((state) => ({ toasts: [...state.toasts, { id: (seq += 1), message, ttl }] })),
  dismiss: (id) => set((state) => ({ toasts: state.toasts.filter((toast) => toast.id !== id) })),
}));

/** Fire a toast imperatively (from keymap handlers, fs-change listener, …). */
export function toast(message: string, ttl?: number): void {
  useToastStore.getState().push(message, ttl);
}

function ToastItem({ toast: item }: { toast: Toast }) {
  const dismiss = useToastStore((state) => state.dismiss);
  useEffect(() => {
    const handle = window.setTimeout(() => dismiss(item.id), item.ttl);
    return () => window.clearTimeout(handle);
  }, [item.id, item.ttl, dismiss]);
  return (
    <div className="toast" role="status">
      {item.message}
    </div>
  );
}

export function Toasts() {
  const toasts = useToastStore((state) => state.toasts);
  if (toasts.length === 0) return null;
  return (
    <div className="toasts" aria-live="polite">
      {toasts.map((item) => (
        <ToastItem key={item.id} toast={item} />
      ))}
    </div>
  );
}
