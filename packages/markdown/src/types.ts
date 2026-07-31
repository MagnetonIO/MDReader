import type { Root as HastRoot } from 'hast';

/**
 * Parsed frontmatter. On success this is the structured YAML mapping; when the
 * YAML is malformed the extractor is tolerant and returns {@link FrontmatterError}
 * instead of throwing, so the body still renders.
 */
export type FrontmatterData = Record<string, unknown> | FrontmatterError;

/** Tolerant-parse result for malformed frontmatter. */
export interface FrontmatterError {
  /** The raw, unparsed YAML block. */
  raw: string;
  /** A human-readable description of why the YAML failed to parse. */
  error: string;
}

/** Type guard distinguishing the malformed-frontmatter case. */
export function isFrontmatterError(value: FrontmatterData | null): value is FrontmatterError {
  return (
    value !== null &&
    typeof value === 'object' &&
    'error' in value &&
    'raw' in value &&
    typeof (value as FrontmatterError).error === 'string' &&
    typeof (value as FrontmatterError).raw === 'string'
  );
}

/** One extracted heading in the document outline. */
export interface OutlineItem {
  /** Heading level, 1..6. */
  depth: number;
  /** Rendered text content of the heading. */
  text: string;
  /** Slug/id matching the heading element's DOM `id` (github-slugger). */
  slug: string;
  /** 1-based source line the heading starts on. */
  lineStart: number;
}

/** Coarse document statistics used for render/scheduling decisions. */
export interface DocStats {
  /** Number of top-level (block) elements in the rendered tree. */
  blockCount: number;
  /** UTF-8 byte length of the source. */
  byteLength: number;
}

/** The single result shape produced by the rendering pipeline. */
export interface ParsedDoc {
  /** The sanitized, KaTeX-rendered, line-stamped hast tree. */
  hast: HastRoot;
  /** Structured frontmatter, or null when the document has none. */
  frontmatter: FrontmatterData | null;
  /** Flat, in-order list of headings. */
  outline: OutlineItem[];
  /** Document statistics. */
  stats: DocStats;
}

/** Options accepted by the parser. */
export interface ParseOptions {
  /**
   * Color used by KaTeX when it renders a malformed expression. rehype-katex
   * never throws on bad math; it renders the source in this color instead.
   * Defaults to KaTeX's own default when omitted.
   */
  katexErrorColor?: string;
}
