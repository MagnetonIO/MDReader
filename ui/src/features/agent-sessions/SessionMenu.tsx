import { useEffect, useRef, type KeyboardEvent } from 'react';

export interface MenuAction { label: string; run(): void }

/** A small keyboard menu; closing always hands focus back to the session row. */
export function SessionMenu({ label, actions, onClose }: { label: string; actions: MenuAction[]; onClose(): void }) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    ref.current?.querySelector<HTMLElement>('[role="menuitem"]')?.focus();
    const dismiss = (event: PointerEvent) => { if (!ref.current?.contains(event.target as Node)) onClose(); };
    document.addEventListener('pointerdown', dismiss, true);
    return () => document.removeEventListener('pointerdown', dismiss, true);
  }, [onClose]);
  function onKeyDown(event: KeyboardEvent) {
    const items = [...(ref.current?.querySelectorAll<HTMLElement>('[role="menuitem"]') ?? [])];
    const index = items.indexOf(document.activeElement as HTMLElement);
    if (event.key === 'Escape' || event.key === 'Tab') onClose();
    else if (event.key === 'ArrowDown') items[(index + 1) % items.length]?.focus();
    else if (event.key === 'ArrowUp') items[(index - 1 + items.length) % items.length]?.focus();
    else if (event.key === 'Home') items[0]?.focus();
    else if (event.key === 'End') items[items.length - 1]?.focus();
    else return;
    event.preventDefault(); event.stopPropagation();
  }
  return <div ref={ref} role="menu" aria-label={label} className="agent-sessions__menu" onKeyDown={onKeyDown}>
    {actions.map(action => <button key={action.label} type="button" role="menuitem" tabIndex={-1}
      onClick={event => { event.stopPropagation(); onClose(); action.run(); }}>{action.label}</button>)}
  </div>;
}
