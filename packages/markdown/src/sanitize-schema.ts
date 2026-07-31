import { defaultSchema } from 'rehype-sanitize';
import type { Options as SanitizeSchema } from 'rehype-sanitize';

type AttrList = NonNullable<NonNullable<SanitizeSchema['attributes']>[string]>;

const baseAttributes = defaultSchema.attributes ?? {};

/**
 * True for a hast `data-*` property name (camelCased, e.g. `dataFootnotes`,
 * `dataFootnoteRef`, `dataFootnoteBackref`). `className`/`href`/aria names do not
 * match, so filtering by this keeps them.
 */
const isDataAttrName = (name: unknown): boolean =>
  typeof name === 'string' && /^data[A-Z0-9]/.test(name);

/**
 * Drop every raw `data-*` grant from one element's allow-list. An entry is
 * either a bare name (`'dataFootnotes'`) or a `[name, …matchers]` tuple
 * (`['className', 'data-footnote-backref']` — a *class value*, not a data attr,
 * so it is kept). Only the attribute NAME is inspected.
 */
const withoutDataAttrs = (list: AttrList): AttrList =>
  list.filter((entry) => !isDataAttrName(Array.isArray(entry) ? entry[0] : entry));

/**
 * The GitHub default's inherited per-element allow-lists with EVERY raw `data-*`
 * grant stripped — notably `section`'s `data-footnotes` and `a`'s
 * `data-footnote-ref` / `data-footnote-backref`. The keep-list for raw content is
 * therefore empty of `data-*`; the only `data-*` this schema permits are the
 * custom `<wiki-link>`'s, granted explicitly below. Pipeline-added `data-*` (the
 * `data-line-*` provenance stamps) are written AFTER sanitize, so they need no
 * schema grant.
 */
const baseAttributesNoData: Record<string, AttrList> = Object.fromEntries(
  Object.entries(baseAttributes).map(([tag, list]) => [
    tag,
    withoutDataAttrs((list ?? []) as AttrList),
  ]),
);

const globalAttributes: AttrList = baseAttributesNoData['*'] ?? [];

/**
 * The single `user-content-` namespace applied to every surviving `id`. The
 * sanitizer is the sole prefix authority (via {@link readerSanitizeSchema}'s
 * `clobberPrefix`), rehype-slug prefixes heading ids with the same value, and
 * `rehypeReconcileFragments` rewrites references to match. Exported so those
 * three steps share one source of truth.
 */
export const USER_CONTENT_PREFIX = 'user-content-';

/**
 * The GitHub default puts `id` and `name` in the global (`*`) allow-list. We
 * drop the blanket `id` grant (re-allowing it only on the elements that
 * legitimately carry one: heading slug targets and the anchors/definitions GFM
 * footnotes generate) and drop `name` entirely — it is the legacy pre-`id`
 * anchor mechanism (`<a name>`), and nothing in the corpus or the product needs
 * it. Removing it means there is no dangling `name` for reconciliation to worry
 * about: only `id` participates in intra-document fragment resolution.
 */
const globalWithoutIdOrName: AttrList = globalAttributes.filter(
  (attr) => attr !== 'id' && attr !== 'name',
);

const withId = (tag: string): AttrList => [...(baseAttributesNoData[tag] ?? []), 'id'];

/**
 * Sanitizer schema for the reader pipeline. Starts from the GitHub-flavoured
 * default and narrows/extends it to exactly what the reader needs — no blanket
 * `className` or `id` on every element:
 *
 *  - `className` is allowed only on `<code>`, and only for the values the reader
 *    reads before this stage: `language-*` (syntax highlighting, incl.
 *    `language-mermaid`) and the `math-inline` / `math-display` markers that
 *    rehype-katex uses to pick inline vs display mode. Everything KaTeX renders
 *    (`katex*` spans) is produced *after* sanitize and needs no schema grant.
 *  - `id` is allowed only on headings (`h1`–`h6`, the slug/scroll-spy targets)
 *    and on the `<a>` / `<li>` elements GFM footnotes anchor. Everything else's
 *    `id` is dropped outright (not merely namespaced), which is the anti-DOM-
 *    clobbering protection that matters here.
 *  - `name` is not allowed anywhere (dropped from the global list above), so the
 *    legacy `<a name>` anchor mechanism is gone and nothing but `id` can seed an
 *    intra-document fragment target.
 *  - `clobberPrefix` is `user-content-` and `clobber` is `['id']`, so the
 *    sanitizer is the SINGLE authority that namespaces every surviving `id`
 *    attribute (a raw `id="constructor"` DOM-clobbering payload therefore arrives
 *    prefixed, never bare). remark-rehype is run with `clobberPrefix: ''` so
 *    footnote ids/references arrive un-prefixed and this schema prefixes each id
 *    exactly once; `rehypeUniqueIds` then gives any duplicated id a deterministic
 *    suffix (origin-blind, first-wins), and `rehypeReconcileFragments` rewrites
 *    the matching `href`/`aria-*` references (which the sanitizer does not touch)
 *    to the surviving prefixed id. The invariant is not "an id is never doubly
 *    prefixed" — a raw `id="user-content-x"` legitimately sanitizes to
 *    `user-content-user-content-x` (prefixing is unconditional) and reconcile
 *    prepends once to match it — but rather: every id is unique after dedup, so
 *    every internal fragment reference that MATCHES a final id resolves to
 *    exactly one element. A reference matching no id — a genuine dead anchor or
 *    an external link's `#fragment` — is not guaranteed to resolve at all; it is
 *    left dangling, unchanged, by design.
 *  - NO raw `data-*` attribute is allowed. The GitHub default granted
 *    `data-footnotes` (on `<section>`) and `data-footnote-ref` /
 *    `data-footnote-backref` (on `<a>`); those are stripped here (see
 *    {@link baseAttributesNoData}) so a raw `<section data-footnotes>` /
 *    `<a data-footnote-ref>` cannot forge the *shape* of a footnote — nothing
 *    downstream makes a trust decision on those attrs (dedup is origin-blind, the
 *    outline is position-matched), and this keeps the DOM honest besides. Genuine
 *    GFM footnotes still render and resolve (their ids/hrefs/aria are what the
 *    pipeline uses); they simply no longer carry the `data-footnote*` markers,
 *    which nothing in the reader consumes. If a future styling/test need arises,
 *    re-add them in a post-sanitize step keyed on the regenerated footnote
 *    structure — do NOT widen this schema.
 *  - `data-line-start` / `data-line-end` provenance stamps are NOT allowed here
 *    (they are `data-*`, stripped like the rest): a raw `<div data-line-start=…>`
 *    cannot forge provenance. The pipeline is the only stamp source —
 *    `rehypeStampBlockMath` (which runs AFTER this sanitize pass) and the final
 *    `stampSourceLines` pass write every real stamp downstream of here, from true
 *    source positions.
 *  - the custom `<wiki-link>` element and its `data-*` attributes are the sole
 *    permitted `data-*`, granted EXPLICITLY (not inherited) and with `className`
 *    restricted to the single `wikilink` token — NOT an open grant, so a raw
 *    `<wiki-link class="katex-display">` cannot smuggle a math class in.
 *
 * Script/`on*` handlers, `javascript:` URLs, and other unsafe constructs are
 * still stripped by the inherited default schema.
 */
export const readerSanitizeSchema: SanitizeSchema = {
  ...defaultSchema,
  clobberPrefix: USER_CONTENT_PREFIX,
  clobber: ['id'],
  tagNames: [...(defaultSchema.tagNames ?? []), 'wiki-link'],
  attributes: {
    // Inherited per-element grants, with every raw `data-*` removed.
    ...baseAttributesNoData,
    // No blanket `id`; provenance/`data-*` stamps are stripped (the pipeline owns
    // its stamps post-sanitize); `name` is dropped entirely (legacy anchor
    // mechanism, so only `id` seeds a fragment target).
    '*': globalWithoutIdOrName,
    // `language-*` (default) plus the KaTeX display-mode markers.
    code: [['className', /^language-./, 'math-inline', 'math-display']],
    h1: withId('h1'),
    h2: withId('h2'),
    h3: withId('h3'),
    h4: withId('h4'),
    h5: withId('h5'),
    h6: withId('h6'),
    a: withId('a'),
    li: withId('li'),
    'wiki-link': [
      ['className', 'wikilink'],
      'dataWikilink',
      'dataTarget',
      'dataHeading',
      'dataLabel',
    ],
  },
};
