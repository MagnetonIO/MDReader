import type { OutlineItem } from './types.js';
import { USER_CONTENT_PREFIX } from './sanitize-schema.js';

/** Resolve a decoded heading reference against actual rendered outline IDs. */
export function resolveHeading(outline: readonly OutlineItem[], target: string): OutlineItem | null {
  const exact = outline.find((item) => item.slug === target)
    ?? outline.find((item) => item.slug === `${USER_CONTENT_PREFIX}${target}`);
  if (exact) return exact;
  const matches = outline.filter((item) => item.text.toLowerCase() === target.toLowerCase());
  return matches.length === 1 ? matches[0]! : null;
}
