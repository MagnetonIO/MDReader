import { createContext, useCallback, useContext, useLayoutEffect, useRef, useState, type FocusEvent, type KeyboardEvent } from 'react';

/** Roving focus for one ARIA tree: exactly one treeitem is tabbable at a time. */
const ActiveItem = createContext<string | null>(null);
export const TreeFocusProvider = ActiveItem.Provider;

export function useTreeItem(id: string) {
  const active = useContext(ActiveItem);
  return { 'data-tree-id': id, tabIndex: active === id ? 0 : -1 } as const;
}

const items = (tree: HTMLElement) => [...tree.querySelectorAll<HTMLElement>('[role="treeitem"]')];
/** The row inside a treeitem carries its primary action (toggle or open). */
const activate = (item: HTMLElement) => item.querySelector<HTMLElement>(':scope > [data-tree-row]')?.click();

export function useTreeNavigation() {
  const ref = useRef<HTMLUListElement>(null);
  const [active, setActive] = useState<string | null>(null);
  useLayoutEffect(() => {
    const tree = ref.current;
    if (!tree) return;
    const all = items(tree);
    if (!all.some(item => item.dataset.treeId === active)) setActive(all[0]?.dataset.treeId ?? null);
  });
  const onFocus = useCallback((event: FocusEvent<HTMLElement>) => {
    const item = (event.target as HTMLElement).closest<HTMLElement>('[role="treeitem"]');
    if (item?.dataset.treeId) setActive(item.dataset.treeId);
  }, []);
  const onKeyDown = useCallback((event: KeyboardEvent<HTMLElement>) => {
    const tree = ref.current, item = event.target as HTMLElement;
    if (!tree || item.getAttribute('role') !== 'treeitem') return;
    const all = items(tree), index = all.indexOf(item), expanded = item.getAttribute('aria-expanded');
    let next: HTMLElement | null | undefined;
    switch (event.key) {
      case 'ArrowDown': next = all[index + 1]; break;
      case 'ArrowUp': next = all[index - 1]; break;
      case 'Home': next = all[0]; break;
      case 'End': next = all[all.length - 1]; break;
      case 'ArrowRight':
        if (expanded === 'false') activate(item); else if (expanded === 'true') next = all[index + 1];
        break;
      case 'ArrowLeft':
        if (expanded === 'true') activate(item); else next = item.parentElement?.closest<HTMLElement>('[role="treeitem"]');
        break;
      case 'Enter': case ' ': activate(item); break;
      default: return;
    }
    event.preventDefault();
    next?.focus();
  }, []);
  return { ref, active, treeProps: { ref, onFocus, onKeyDown } };
}
