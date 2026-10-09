import type { Element, ElementContent } from 'hast';

/** Match only final, sanitized IDs. Hosts decode URI fragments once at ingress;
 * native link receipts already carry decoded heading text. */
export function fragmentTarget(blocks: readonly Element[], fragment: string): { block: number; id: string } | null {
  if (fragment.length > 4096 || /[\u0000-\u001f\u007f]/.test(fragment)) return null;
  if (!fragment) return blocks.length ? { block: 0, id: '' } : null;
  const ids = new Map<string, number>();
  const visit = (node: ElementContent, block: number) => {
    if (node.type !== 'element') return;
    const id = node.properties.id;
    if (typeof id === 'string' && !ids.has(id)) ids.set(id, block);
    for (const child of node.children) visit(child, block);
  };
  blocks.forEach((block, index) => visit(block, index));
  for (const id of [fragment, `user-content-${fragment}`]) {
    const block = ids.get(id);
    if (block !== undefined) return { block, id };
  }
  return null;
}
