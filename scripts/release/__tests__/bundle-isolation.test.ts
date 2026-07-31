/**
 * Boundary guard: CodeMirror (the desktop light-editing dependency) must never
 * reach the web reader bundle. This resolves the ACTUAL import graph the web
 * bundle is built from — starting at the web app's entry files and following every
 * import/export specifier transitively, including through the workspace barrels
 * (`@mdreader/ui`, `@mdreader/markdown`) — and fails if ANY reachable module
 * imports a CodeMirror/Lezer package.
 *
 * A package.json check would be vacuous (pnpm hoists deps, so a direct import that
 * the manifest never declares still resolves). A three-hardcoded-directory scan
 * would miss a barrel that re-exports a CM-importing module from elsewhere. This
 * graph walk catches BOTH: it goes red on a direct `import '@codemirror/state'` in
 * the web app, and on one hidden behind a `@mdreader/ui` barrel re-export
 * (verified red against a crafted barrel-hidden import).
 */

import { existsSync, readFileSync, readdirSync, statSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

function repoRoot(): string {
  let dir = dirname(fileURLToPath(import.meta.url));
  for (let i = 0; i < 12; i += 1) {
    if (existsSync(join(dir, 'pnpm-workspace.yaml'))) return dir;
    dir = dirname(dir);
  }
  throw new Error('could not locate the monorepo root');
}

const ROOT = repoRoot();
const CM = /^(@codemirror\/|@lezer\/|codemirror$)/;

/** Try a set of extension/index candidates for an extensionless path. */
function resolveFile(pathNoExt: string): string | null {
  for (const ext of ['.ts', '.tsx', '.js', '.jsx', '.mjs']) {
    const p = pathNoExt + ext;
    if (existsSync(p) && statSync(p).isFile()) return p;
  }
  for (const ext of ['/index.ts', '/index.tsx', '/index.js']) {
    const p = pathNoExt + ext;
    if (existsSync(p) && statSync(p).isFile()) return p;
  }
  if (existsSync(pathNoExt) && statSync(pathNoExt).isFile()) return pathNoExt;
  return null;
}

/** Resolve a specifier from `importer` to a workspace SOURCE file, or null when it
 *  is external (react/next/node — not part of the workspace graph). */
function resolve(importer: string, spec: string): string | null {
  if (spec.startsWith('.')) {
    const raw = join(dirname(importer), spec);
    return resolveFile(raw.replace(/\.(js|jsx|ts|tsx|mjs)$/, ''));
  }
  // The web tsconfig defines `@/*` → `./*` (rooted at apps/web). Resolve it as a
  // LOCAL file and follow it — a route importing CodeMirror via `@/…` is in the
  // bundle just the same.
  if (spec.startsWith('@/')) {
    const raw = join(ROOT, 'apps/web', spec.slice(2));
    return resolveFile(raw.replace(/\.(js|jsx|ts|tsx|mjs)$/, ''));
  }
  const workspace = /^@mdreader\/([^/]+)(?:\/(.+))?$/.exec(spec);
  if (workspace) {
    const [, pkg, sub] = workspace;
    if (!pkg || pkg === 'config') return null; // build-only tooling, never bundled
    const base = join(ROOT, 'packages', pkg, 'src');
    const noExt = (sub ?? 'index').replace(/\.(js|jsx|ts|tsx)$/, '');
    // Follow the package's export map for known subpaths, else best-effort by name.
    return (
      resolveFile(join(base, noExt)) ??
      resolveFile(join(base, `${noExt}-client`)) ??
      resolveFile(join(base, noExt, 'index'))
    );
  }
  return null; // external (react / next / node builtin)
}

/** Every import/export/dynamic-import specifier in a source file. */
function specifiers(text: string): string[] {
  const out: string[] = [];
  const push = (re: RegExp) => {
    for (const match of text.matchAll(re)) out.push(match[1]!);
  };
  push(/\bfrom\s*['"]([^'"]+)['"]/g); // import … from '…' / export … from '…'
  push(/\bimport\s*['"]([^'"]+)['"]/g); // side-effect import '…'
  push(/\bimport\(\s*['"]([^'"]+)['"]\s*\)/g); // dynamic import('…')
  return out;
}

/** Walk the import graph from `entries`, returning every reachable CodeMirror/
 *  Lezer import (as `importer → specifier`). */
function reachableCmImports(entries: string[]): string[] {
  const visited = new Set<string>();
  const queue = [...entries];
  const offenders: string[] = [];
  while (queue.length > 0) {
    const file = queue.pop()!;
    if (visited.has(file)) continue;
    visited.add(file);
    for (const spec of specifiers(readFileSync(file, 'utf8'))) {
      if (CM.test(spec)) {
        offenders.push(`${file} → ${spec}`);
        continue;
      }
      const target = resolve(file, spec);
      if (target && !visited.has(target)) queue.push(target);
    }
  }
  return offenders;
}

/** Every App Router entrypoint under `dir` — RECURSIVELY, since each nested route
 *  (page/layout/route/…) is an independent bundle entrypoint that could import
 *  CodeMirror, not just the top-level ones. */
function routeEntrypoints(dir: string): string[] {
  const ENTRYPOINT = /^(page|layout|route|template|default|loading|error|not-found)\.(ts|tsx)$/;
  const out: string[] = [];
  const walk = (current: string) => {
    for (const entry of readdirSync(current, { withFileTypes: true })) {
      if (entry.name === 'node_modules' || entry.name === '.next') continue;
      const full = join(current, entry.name);
      if (entry.isDirectory()) walk(full);
      else if (ENTRYPOINT.test(entry.name) && !/\.test\.(ts|tsx)$/.test(entry.name)) out.push(full);
    }
  };
  walk(dir);
  return out;
}

const webEntries = () => routeEntrypoints(join(ROOT, 'apps/web/app'));
const FIXTURES = join(ROOT, 'apps/web/__fixtures__/routes');

function deps(manifestPath: string): string[] {
  const pkg = JSON.parse(readFileSync(manifestPath, 'utf8')) as {
    dependencies?: Record<string, string>;
    devDependencies?: Record<string, string>;
    peerDependencies?: Record<string, string>;
  };
  return [
    ...Object.keys(pkg.dependencies ?? {}),
    ...Object.keys(pkg.devDependencies ?? {}),
    ...Object.keys(pkg.peerDependencies ?? {}),
  ];
}

describe('CodeMirror stays out of the web reader bundle', () => {
  it('no module reachable from the web app entry imports CodeMirror/Lezer', () => {
    // Sanity: the walk actually reaches the shared barrel (a green result on an
    // empty graph would be vacuous).
    const entries = webEntries();
    expect(entries.length).toBeGreaterThan(0);
    expect(reachableCmImports(entries)).toEqual([]);
  });

  it('the walk really traverses @mdreader/ui (guards against a vacuous pass)', () => {
    // Reaching the shared reader barrel from the web entry is the whole point; if
    // resolution silently stopped at the package boundary this would be empty.
    const reached = (() => {
      const visited = new Set<string>();
      const queue = [...webEntries()];
      while (queue.length) {
        const file = queue.pop()!;
        if (visited.has(file)) continue;
        visited.add(file);
        for (const spec of specifiers(readFileSync(file, 'utf8'))) {
          const target = resolve(file, spec);
          if (target && !visited.has(target)) queue.push(target);
        }
      }
      return visited;
    })();
    expect([...reached].some((f) => f.includes(join('packages', 'ui', 'src')))).toBe(true);
  });

  it('neither the web app nor @mdreader/ui DECLARES a CodeMirror/Lezer dependency', () => {
    expect(deps(join(ROOT, 'apps/web/package.json')).filter((d) => CM.test(d))).toEqual([]);
    expect(deps(join(ROOT, 'packages/ui/package.json')).filter((d) => CM.test(d))).toEqual([]);
  });

  it('the desktop app package IS where CodeMirror lives (positive control)', () => {
    expect(
      deps(join(ROOT, 'apps/mdreader/ui/package.json')).some((d) => d.startsWith('@codemirror/')),
    ).toBe(true);
  });
});

// A committed fixture tree (apps/web/__fixtures__/routes, never bundled) proves the
// guard's teeth WITHOUT tripping the real "app is clean" assertion above.
describe('the guard flags CodeMirror hidden in a nested route / behind the @/ alias (fixture)', () => {
  it('FLAGS a CodeMirror import reachable only through a NESTED route', () => {
    // __fixtures__/routes/nested/page.tsx → ../cm-via-relative → @codemirror/state.
    // Reverting routeEntrypoints to a non-recursive scan misses the nested route.
    const offenders = reachableCmImports(routeEntrypoints(FIXTURES));
    expect(offenders.some((o) => o.includes('cm-via-relative'))).toBe(true);
  });

  it('FLAGS a CodeMirror import reachable only through the @/ path alias', () => {
    // __fixtures__/routes/page.tsx → @/__fixtures__/routes/cm-via-alias → @lezer/…
    // Reverting the `@/` resolution treats it as external and misses it.
    const offenders = reachableCmImports(routeEntrypoints(FIXTURES));
    expect(offenders.some((o) => o.includes('cm-via-alias'))).toBe(true);
  });
});
