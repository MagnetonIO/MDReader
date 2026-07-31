import { parse as parseYaml } from 'yaml';
import { visit } from 'unist-util-visit';
import type { Root as MdastRoot, Yaml } from 'mdast';

import type { FrontmatterData } from './types.js';

/**
 * Parse a single raw YAML frontmatter block into structured data.
 *
 * Tolerance contract: malformed YAML never throws. Instead this returns
 * `{ raw, error }` so the caller can surface a warning while still rendering the
 * body. An empty block returns null ("no metadata"). A top-level scalar or list
 * is wrapped as `{ value }` so callers always get an object on success.
 */
export function parseFrontmatterBlock(raw: string): FrontmatterData | null {
  if (raw.trim() === '') return null;
  try {
    const parsed = parseYaml(raw) as unknown;
    if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) {
      return parsed as Record<string, unknown>;
    }
    if (parsed === null || parsed === undefined) return null;
    return { value: parsed };
  } catch (cause) {
    return {
      raw,
      error: cause instanceof Error ? cause.message : String(cause),
    };
  }
}

/**
 * Extract and parse the frontmatter from a parsed mdast tree. remark-frontmatter
 * emits a `yaml` node for a leading `---` block; there is at most one.
 */
export function extractFrontmatter(tree: MdastRoot): FrontmatterData | null {
  let frontmatter: FrontmatterData | null = null;
  visit(tree, 'yaml', (node: Yaml) => {
    frontmatter = parseFrontmatterBlock(node.value);
    return false; // only the first frontmatter block
  });
  return frontmatter;
}
