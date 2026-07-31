import { visit } from 'unist-util-visit';
import type { Element, Root as HastRoot } from 'hast';

import { USER_CONTENT_PREFIX } from './sanitize-schema.js';

/**
 * hast id-reference attributes whose value is an ARRAY of bare id names (aria
 * references). `href` is handled separately because it is a single `#`-fragment
 * string, not an array.
 */
const ARIA_ID_REFERENCE_PROPS = ['ariaDescribedBy', 'ariaLabelledBy'] as const;

/**
 * Post-sanitize rehype step: reconcile internal references with the namespaced
 * ids.
 *
 * The sanitizer is the single prefix authority — it clobber-prefixes every
 * surviving `id` with `user-content-`, and rehype-slug prefixes heading ids the
 * same way — but neither rewrites the *references* to those ids. So after
 * prefixing, GFM footnote links/back-refs, footnote `aria-describedby`, and any
 * legitimate raw or markdown intra-document anchor still point at the pre-prefix
 * name and would dangle. `rehypeUniqueIds` runs before this step, so the id set
 * indexed here is FINAL: every id is unique (any duplicate has already been given
 * a deterministic suffix), so a `#X` reference resolves to the single element
 * that owns the matching final id.
 *
 * This step fixes references uniformly, after every id is final: for each
 * internal fragment reference (`href="#X"`) and aria id-reference
 * (`aria-describedby`/`aria-labelledby`, stored by hast as ARRAYS), if the bare
 * target `X` does not resolve but `user-content-X` exists as an id, the reference
 * is rewritten to the prefixed id. One mechanism reconciles footnotes AND raw
 * anchors alike.
 *
 * The honest invariant: an internal fragment reference that matches a final id
 * resolves to that id's first occurrence in document order; a reference with no
 * matching final id is preserved unchanged (dangling is allowed — external links
 * and genuine dead anchors are left exactly as authored). The "first occurrence"
 * clause is not a tiebreak this code has to arbitrate: `rehypeUniqueIds` has
 * already made every id unique before this step runs, so the id set built below
 * has at most one element per id and multiplicity is impossible here. It checks
 * final-id-set membership first, so it is idempotent — a reference that already
 * resolves is left untouched (never re-prefixed), which is also why a
 * legitimately double-prefixed target survives: a raw `id="user-content-x"`
 * sanitizes to `user-content-user-content-x`, and a `#user-content-x` reference
 * is prefixed exactly once, to that id.
 */
export function rehypeReconcileFragments() {
  return (tree: HastRoot): void => {
    // 1. Every final id present in the tree (post-sanitize/slug, so prefixed).
    //    `rehypeUniqueIds` ran first, so each id appears on exactly one element:
    //    the Set therefore models the full final id set with no multiplicity.
    const ids = new Set<string>();
    visit(tree, 'element', (node: Element) => {
      const id = node.properties?.id;
      if (typeof id === 'string') ids.add(id);
    });

    // Resolve a bare reference to the id that actually exists. Prefer an
    // already-resolving reference (idempotent — returns undefined so callers
    // leave it) over the prefixed candidate; return undefined when neither the
    // bare nor the prefixed id exists (external/dangling — leave it).
    const resolve = (ref: string): string | undefined => {
      if (ids.has(ref)) return undefined;
      const prefixed = USER_CONTENT_PREFIX + ref;
      return ids.has(prefixed) ? prefixed : undefined;
    };

    // 2. Rewrite references that only resolve under the prefix.
    visit(tree, 'element', (node: Element) => {
      const props = node.properties;
      if (!props) return;

      const href = props.href;
      if (typeof href === 'string' && href.startsWith('#')) {
        const target = resolve(href.slice(1));
        if (target) props.href = `#${target}`;
      }

      for (const prop of ARIA_ID_REFERENCE_PROPS) {
        const value = props[prop];
        if (!Array.isArray(value)) continue;
        props[prop] = value.map((item) =>
          typeof item === 'string' ? (resolve(item) ?? item) : item,
        );
      }
    });
  };
}
