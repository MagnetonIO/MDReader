/** Semantic visibility matters for native disclosures: their closed descendants
 * exist in the DOM but cannot receive keyboard focus. Keep each visible summary. */
export function readerSheetFocusable(dialog: HTMLElement): HTMLElement[] {
  return Array.from(dialog.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled), summary, a[href], [tabindex="0"]'))
    .filter(element => {
      if (element.tabIndex === -1 || element.matches(':disabled') || element.closest('[hidden], [inert], [aria-hidden="true"]')) return false;
      for (let ancestor = element.parentElement; ancestor && ancestor !== dialog; ancestor = ancestor.parentElement) {
        if (ancestor instanceof HTMLDetailsElement && !ancestor.open) {
          const summary = Array.from(ancestor.children).find(child => child.tagName === 'SUMMARY');
          if (!summary?.contains(element)) return false;
        }
      }
      return true;
    })
    .sort((left, right) => left.compareDocumentPosition(right) & Node.DOCUMENT_POSITION_FOLLOWING ? -1 : 1);
}
