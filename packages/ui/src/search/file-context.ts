/** Display/search context only. Never use a normalized path to open a file. */
export interface FileContext {
  /** Only present for lexical descendants of an explicitly supplied workspace. */
  relativePath: string | null;
  parentLabel: string;
}

function normalized(path: string): string {
  return path.replace(/\\/g, '/').replace(/\/+$/, '');
}

export function fileContext(path: string, workspaceRoot?: string | null): FileContext {
  const target = normalized(path);
  const root = workspaceRoot ? normalized(workspaceRoot) : null;
  // Drive/UNC paths compare case-insensitively for display membership only.
  const windows = /^(?:[a-z]:\/|\/\/)/i.test(target);
  const compare = (value: string) => windows ? value.toLowerCase() : value;
  const prefix = root === '' && workspaceRoot ? '/' : root ? `${root}/` : null;
  const candidate = prefix && compare(target).startsWith(compare(prefix))
    ? target.slice(prefix.length) : null;
  // Do not resolve traversal or infer a filesystem grant from string prefixes.
  const relativePath = candidate && candidate.split('/').every((part) => part && part !== '.' && part !== '..')
    ? candidate : null;
  const parents = (relativePath ?? target).split('/').slice(0, -1).filter(Boolean);
  const shortParent = parents.slice(-2).join('/');
  return {
    relativePath,
    parentLabel: relativePath
      ? parents.length ? `${parents.length > 2 ? '…/' : ''}${shortParent}` : 'Workspace root'
      : `${workspaceRoot ? 'Outside workspace' : 'File'}${shortParent ? ` · ${shortParent}` : ''}`,
  };
}
