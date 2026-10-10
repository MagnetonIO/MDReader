import { useLayoutEffect } from 'react';
import type { RefObject } from 'react';

/** The sheet is bounded to its reader, but its modal keyboard/assistive scope
 * includes the tablet Library and phone tab bar. Preserve existing host flags. */
export function useReaderSheetIsolation(frame: RefObject<HTMLElement | null>, open: boolean) {
  useLayoutEffect(() => {
    if (!open || !frame.current) return;
    const shell = frame.current.closest('.mobile-shell');
    if (!shell) return;
    const saved: { element: HTMLElement; inert: boolean; hidden: string | null }[] = [];
    let child: Element = frame.current;
    while (child !== shell && child.parentElement) {
      for (const sibling of child.parentElement.children) {
        if (sibling === child || !(sibling instanceof HTMLElement)) continue;
        saved.push({ element: sibling, inert: sibling.hasAttribute('inert'), hidden: sibling.getAttribute('aria-hidden') });
        sibling.setAttribute('inert', ''); sibling.setAttribute('aria-hidden', 'true');
      }
      child = child.parentElement;
    }
    return () => {
      for (const { element, inert, hidden } of saved) {
        // Do not overwrite another host's later ownership change.
        if (element.hasAttribute('inert') && !inert) element.removeAttribute('inert');
        if (element.getAttribute('aria-hidden') === 'true') {
          if (hidden === null) element.removeAttribute('aria-hidden');
          else element.setAttribute('aria-hidden', hidden);
        }
      }
    };
    // Reconcile after each frame commit: a phone/tablet reflow can add siblings
    // while the same sheet remains open. Cleanup restores only the prior flags.
  });
}
