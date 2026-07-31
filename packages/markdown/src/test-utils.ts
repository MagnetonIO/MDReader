import { readdirSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

/** Absolute path to a file in the committed `fixtures/corpus` tree. */
export function corpusPath(relative: string): string {
  // This file lives in packages/markdown/src; the corpus is at the repo root.
  return fileURLToPath(new URL(`../../../fixtures/corpus/${relative}`, import.meta.url));
}

/** Read a corpus fixture as UTF-8 text. */
export function readCorpus(relative: string): string {
  return readFileSync(corpusPath(relative), 'utf8');
}

/**
 * Recursively list every markdown file (`.md` / `.markdown`) in the corpus,
 * as paths relative to `fixtures/corpus`, sorted for deterministic test order.
 */
export function listCorpusMarkdown(): string[] {
  const root = corpusPath('.');
  const found: string[] = [];
  const walk = (dir: string, prefix: string): void => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      const rel = prefix ? `${prefix}/${entry.name}` : entry.name;
      if (entry.isDirectory()) {
        walk(`${dir}/${entry.name}`, rel);
      } else if (/\.(md|markdown)$/.test(entry.name)) {
        found.push(rel);
      }
    }
  };
  walk(root, '');
  return found.sort();
}
