/**
 * Boundary guard: `@tauri-apps/*` must never reach the website bundle. `@mdreader/ui` is
 * platform-agnostic — it takes a `CatdbBackend` + `Platform` via context — and the web app
 * supplies the browser implementations while the desktop app supplies Tauri.
 *
 * HOW IT IS ENFORCED — two complementary layers (no import-graph resolver, no source-map scan,
 * whose encoding/minified-unmapped-chunk holes we could not close):
 *   1. BUILD-TIME ALIAS BAN (primary). next.config.ts aliases every `@tauri-apps/*` request to
 *      guard/tauri-poison.ts, a module that cannot compile, so any STATICALLY-resolved import —
 *      direct, transitive, server or client — fails `next build` by construction.
 *   2. RAW-OUTPUT LITERAL CHECK (belt-and-suspenders). An IGNORED dynamic import bypasses the alias
 *      and leaves the specifier AS WRITTEN in the emitted chunk — a plain `@tauri-apps` literal (no
 *      bundling ⇒ no URL-encoding, no source map, no per-chunk accounting). So the guard also greps
 *      the emitted JS chunks for that literal after a clean build and fails if it appears.
 *
 * These vitest checks pin the alias WIRING deterministically and cheaply. The end-to-end proof that
 * BOTH layers bite — a static import made to fail the build via the poison, an ignored-dynamic
 * import caught as a literal in a real chunk, and the real app building clean with neither — is the
 * negative self-test `scripts/assert-public-site-tauri-ban.mjs`, run as the `guard:no-tauri` gate step (also the
 * revert-RED: remove the alias, or plant a bypassing import, and it goes red).
 */

import { existsSync, readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

import nextConfig from '../next.config';

const HERE = dirname(fileURLToPath(import.meta.url));
const WEB_DIR = join(HERE, '..');
const ROOT = join(WEB_DIR, '..', '..');
const TAURI = /^@tauri-apps\//;

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

describe('@tauri-apps is banned from the web bundle at build time', () => {
  const resolveAlias = (nextConfig.turbopack?.resolveAlias ?? {}) as Record<string, string>;

  it('next.config aliases the @tauri-apps package AND every subpath to the poison module', () => {
    // The bare package and a `/*` glob (covers api/core, api/path, plugin-store, plugin-dialog, …).
    expect(resolveAlias['@tauri-apps']).toBeDefined();
    expect(resolveAlias['@tauri-apps/*']).toBeDefined();
    for (const key of ['@tauri-apps', '@tauri-apps/*']) {
      expect(resolveAlias[key]).toMatch(/guard\/tauri-poison\.ts$/);
    }
  });

  it('the alias target is a genuine POISON module (cannot compile → fails the build)', () => {
    const poison = join(WEB_DIR, 'guard', 'tauri-poison.ts');
    expect(existsSync(poison)).toBe(true);
    // Its power is a static import of a module that intentionally does not exist: whenever the
    // alias routes a real @tauri-apps import here, Turbopack must compile this file and cannot
    // resolve that import, so `next build` fails. If this line were removed the ban would be toothless.
    const src = readFileSync(poison, 'utf8');
    expect(src).toMatch(/import\s+'\.\/__TAURI_IS_BANNED_IN_THE_WEB_BUNDLE__remove_the_import'/);
  });

  it('guard/ is excluded from tsc so the (deliberately uncompilable) poison never breaks a clean build', () => {
    // The poison is compiled ONLY by Turbopack when reached via the alias — never type-checked
    // standalone. Excluding it keeps clean builds/typechecks green while the ban stays armed.
    const tsconfig = JSON.parse(readFileSync(join(WEB_DIR, 'tsconfig.json'), 'utf8')) as {
      exclude?: string[];
    };
    expect(tsconfig.exclude ?? []).toContain('guard');
  });

  it('neither the web app nor @mdreader/ui DECLARES a @tauri-apps dependency', () => {
    // Complementary static guard: an un-resolvable @tauri-apps import also fails the type-check.
    expect(deps(join(ROOT, 'apps/web/package.json')).filter((d) => TAURI.test(d))).toEqual([]);
    expect(deps(join(ROOT, 'packages/ui/package.json')).filter((d) => TAURI.test(d))).toEqual([]);
  });
});
