import { visit } from 'unist-util-visit';
import type { Element, Root as HastRoot } from 'hast';

/**
 * Pipeline-owned id-uniquing pass. Runs AFTER sanitize + slug + footnotes and
 * BEFORE {@link import('./reconcile-fragments.js').rehypeReconcileFragments}, so
 * reconciliation indexes the FINAL id set: internal fragment references that
 * match a final id resolve to its first document-order occurrence; references
 * with no matching id are preserved unchanged (dangling allowed).
 *
 * The dedup is **origin-BLIND and first-wins**: walk the final tree in document
 * order; the first element to claim an id keeps it, and every later duplicate is
 * given a deterministic `-2`/`-3`… suffix (skipping any suffix already taken).
 * There is NO trust input of any kind — no origin classification, no reserved
 * sets, no `data-footnotes` / `data-footnote-ref` / slug / source-line
 * inspection. That absence is the security property: by the time this pass runs,
 * `rehype-raw` has already merged trusted markdown output and untrusted raw HTML
 * into one indistinguishable tree, so ANY per-node signal used to decide "who
 * keeps the bare id" would be forgeable by raw HTML. Document order — read off
 * the merged tree's shape, claimable by anyone but assignable to only one node —
 * is the only unforgeable tiebreak, so it is the only one used.
 *
 * Accepted, documented consequence: a raw element appearing *before* a markdown
 * heading (or before the hoisted footnotes section) can claim the bare id and
 * push the later, "real" element to a `-2` suffix. This is cosmetic, not a
 * correctness bug, because no consumer hard-codes a bare id — the outline reads
 * each heading's FINAL (post-dedup) id off its own matched node, and
 * `rehypeReconcileFragments` reconciles every reference against the FINAL id set.
 * The guarantee this pass makes is therefore exactly: after it runs every id in
 * the tree is unique, so every internal fragment reference that matches an id
 * resolves to exactly one element — never a duplicate collision. This does NOT
 * mean every reference resolves: a reference to a name no element ever claims
 * (a dead anchor, or an external link's `#fragment`) has no matching id before
 * this pass and still has none after — it is left dangling, by design, in
 * {@link import('./reconcile-fragments.js').rehypeReconcileFragments}.
 */
export function rehypeUniqueIds() {
  return (tree: HastRoot): void => {
    const used = new Set<string>();
    visit(tree, 'element', (node: Element) => {
      const props = node.properties;
      const id = props?.id;
      if (!props || typeof id !== 'string') return;
      if (!used.has(id)) {
        used.add(id);
        return;
      }
      let n = 2;
      let candidate = `${id}-${n}`;
      while (used.has(candidate)) {
        n += 1;
        candidate = `${id}-${n}`;
      }
      props.id = candidate;
      used.add(candidate);
    });
  };
}
