export type ReaderPanel = 'Actions' | 'Outline' | 'Appearance' | 'Details';

const dialog = '//section[@role="dialog" and @aria-modal="true"]';

/** XPath text literals must preserve authored heading quotes, not alter them. */
function textLiteral(value: string): string {
  if (!value.includes('"')) return `"${value}"`;
  if (!value.includes("'")) return `'${value}'`;
  return `concat(${value.split('"').map(part => `"${part}"`).join(', \'"\', ')})`;
}

export function readerPanelSelector(panel: ReaderPanel): string {
  return `${dialog}//button[@role="tab" and normalize-space(.)=${textLiteral(panel)}]`;
}

export function readerOutlineSelector(heading: string): string {
  return `${dialog}//section[@role="tabpanel" and not(@hidden)]//nav[@aria-label="Document outline"]//button[normalize-space(.)=${textLiteral(heading)}]`;
}
