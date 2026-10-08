import type { DeviceDocument } from '../bindings';

export type ImportedLink =
  | { state: 'resolved'; document: DeviceDocument; heading: string | null }
  | { state: 'ambiguous' | 'missing' | 'unsupported' | 'invalid' };

/** Imports have selected flat filenames, not inferred original folder authority. */
export function resolveImportedLink(documents: readonly DeviceDocument[], kind: 'relative' | 'wikilink', target: string): ImportedLink {
  if (new TextEncoder().encode(target).length > 4096) return { state: 'invalid' };
  const separator = target.indexOf('#');
  const rawName = separator < 0 ? target : target.slice(0, separator);
  let name: string;
  let heading: string | null;
  try {
    name = decodeURIComponent(rawName);
    heading = separator < 0 ? null : decodeURIComponent(target.slice(separator + 1)) || null;
  } catch { return { state: 'invalid' }; }
  if ([name, heading ?? ''].some((part) => Array.from(part).some((character) => { const code = character.charCodeAt(0); return code < 32 || code >= 127 && code <= 159; })) || /[:\\?]/u.test(name)) return { state: 'invalid' };
  if (kind === 'relative' && name.startsWith('./')) name = name.slice(2);
  if (name.includes('/') || name === '.' || name === '..') return { state: 'unsupported' };
  if (!name) return { state: 'missing' };
  let matches = documents.filter((document) => document.name === name);
  if (!matches.length && kind === 'wikilink' && !name.toLowerCase().endsWith('.md')) {
    matches = documents.filter((document) => document.name === `${name}.md`);
  }
  if (matches.length > 1) return { state: 'ambiguous' };
  return matches[0] ? { state: 'resolved', document: matches[0], heading } : { state: 'missing' };
}
