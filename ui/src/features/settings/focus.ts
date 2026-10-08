const FOCUSABLE = 'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), summary, [tabindex]:not([tabindex="-1"])';

/** Hidden categories and closed disclosures are mounted but must never enter the modal focus loop. */
export function visibleSettingsControls(scope: HTMLElement | null): HTMLElement[] {
  return Array.from(scope?.querySelectorAll<HTMLElement>(FOCUSABLE) ?? []).filter((element) => {
    if (element.closest('[hidden], [inert]')) return false;
    const style = getComputedStyle(element);
    if (style.display === 'none' || style.visibility === 'hidden') return false;
    let parent = element.parentElement;
    while (parent && parent !== scope) {
      if (getComputedStyle(parent).display === 'none' || getComputedStyle(parent).visibility === 'hidden') return false;
      if (parent instanceof HTMLDetailsElement && !parent.open) {
        const summary = Array.from(parent.children).find((child) => child.tagName === 'SUMMARY');
        if (!summary?.contains(element)) return false;
      }
      parent = parent.parentElement;
    }
    return true;
  });
}
