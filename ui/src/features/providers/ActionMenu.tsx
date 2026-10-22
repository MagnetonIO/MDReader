import { Fragment, useEffect, useId, useRef, useState, type KeyboardEvent, type ReactNode } from 'react';

export interface ActionMenuItem {
  id: string;
  label: string;
  description?: string;
  icon?: ReactNode;
  disabled?: boolean;
  danger?: boolean;
  /** Draws a separator before this item. */
  separated?: boolean;
  onSelect(): void;
}

/** Menu button pattern: arrow keys move, Escape closes and focus returns to the trigger. */
export function ActionMenu({ label, icon, items, className }: {
  label: string; icon?: ReactNode; items: ActionMenuItem[]; className?: string;
}) {
  const [open, setOpen] = useState(false);
  const menuId = useId();
  const root = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const menu = useRef<HTMLUListElement>(null);
  useEffect(() => {
    if (!open) return;
    menuItems(menu.current)[0]?.focus();
    const outside = (event: PointerEvent) => { if (!root.current?.contains(event.target as Node)) setOpen(false); };
    document.addEventListener('pointerdown', outside);
    return () => document.removeEventListener('pointerdown', outside);
  }, [open]);
  function close() { setOpen(false); trigger.current?.focus(); }
  function onMenuKey(event: KeyboardEvent<HTMLUListElement>) {
    const nodes = menuItems(menu.current);
    const index = nodes.indexOf(document.activeElement as HTMLElement);
    const move = (next: number) => { event.preventDefault(); nodes[(next + nodes.length) % nodes.length]?.focus(); };
    switch (event.key) {
      case 'Escape': event.preventDefault(); event.stopPropagation(); close(); return;
      case 'ArrowDown': move(index + 1); return;
      case 'ArrowUp': move(index - 1); return;
      case 'Home': move(0); return;
      case 'End': move(nodes.length - 1); return;
      case 'Tab': setOpen(false); return;
    }
  }
  return <div className={`action-menu${className ? ` ${className}` : ''}`} ref={root}>
    <button ref={trigger} type="button" className="action-menu__trigger" aria-haspopup="menu" aria-expanded={open}
      aria-controls={open ? menuId : undefined} aria-label={icon ? label : undefined}
      onClick={() => setOpen(value => !value)}
      onKeyDown={event => { if (event.key === 'ArrowDown' && !open) { event.preventDefault(); setOpen(true); } }}>
      {icon ?? label}
    </button>
    {open ? <ul id={menuId} ref={menu} role="menu" aria-label={label} className="action-menu__list" onKeyDown={onMenuKey}>
      {items.map(item => <Fragment key={item.id}>
        {item.separated ? <li role="separator" className="action-menu__separator" /> : null}
        <li role="none">
          <button type="button" role="menuitem" tabIndex={-1} aria-disabled={item.disabled || undefined}
            className={`action-menu__item${item.danger ? ' action-menu__item--danger' : ''}`}
            onClick={() => { if (item.disabled) return; close(); item.onSelect(); }}>
            {item.icon}
            <span className="action-menu__label">{item.label}
              {item.description ? <span className="action-menu__description">{item.description}</span> : null}</span>
          </button>
        </li>
      </Fragment>)}
    </ul> : null}
  </div>;
}

function menuItems(menu: HTMLUListElement | null): HTMLElement[] {
  return Array.from(menu?.querySelectorAll<HTMLElement>('[role="menuitem"]') ?? []);
}
