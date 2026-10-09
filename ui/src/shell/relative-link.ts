import type { WorkspaceFile } from '../platform/fs';

/** Choose only an already admitted workspace-file entry. Decoding never
 * manufactures a filesystem grant or drops the target fragment. */
export function resolveWorkspaceLink(files: readonly WorkspaceFile[], directory: string, href: string): { file: WorkspaceFile; fragment: string | null } | null {
  const hash = href.indexOf('#');
  let path: string;
  let fragment: string | null;
  try {
    path = decodeURIComponent((hash < 0 ? href : href.slice(0, hash)).split('?')[0]!);
    fragment = hash < 0 ? null : decodeURIComponent(href.slice(hash + 1)) || null;
  } catch { return null; }
  if (!path || /[:\u0000-\u001f\u007f]/.test(path) || path.startsWith('/') || path.startsWith('\\')) return null;
  const parts = directory.replace(/\\/g, '/').replace(/\/+$/, '').split('/');
  for (const part of path.split(/[\\/]+/)) {
    if (!part || part === '.') continue;
    if (part === '..') parts.pop();
    else parts.push(part);
  }
  const resolved = parts.join('/');
  const file = files.find((candidate) => candidate.path.replace(/\\/g, '/') === resolved);
  return file ? { file, fragment } : null;
}
