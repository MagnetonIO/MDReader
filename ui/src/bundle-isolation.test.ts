/**
 * Boundary guard: CodeMirror (the light-editing dependency) must stay in the
 * DESKTOP app package and NEVER reach the web reader bundle. The web app imports
 * only `@mdreader/ui`; as long as neither the web app nor `@mdreader/ui` declares
 * any CodeMirror/Lezer package, the web bundle cannot pull CM6 in.
 *
 * This is an import-graph assertion at the package boundary — it fails the moment
 * someone moves the SourcePane (or any CM6 dependency) into the shared reader
 * package, which is exactly the regression that would bloat the web bundle. The
 * manifests are the authoritative graph: web → @mdreader/ui is the only workspace
 * edge, so a clean @mdreader/ui manifest guarantees a CM6-free web bundle.
 */

import { describe, expect, it } from 'vitest';

import appManifest from '../package.json';
import uiManifest from '../../../../packages/ui/package.json';
import webManifest from '../../../web/package.json';

const EDITOR_DEP = /^(@codemirror\/|@lezer\/|codemirror$)/;

function allDeps(pkg: {
  dependencies?: Record<string, string>;
  devDependencies?: Record<string, string>;
  peerDependencies?: Record<string, string>;
}): string[] {
  return [
    ...Object.keys(pkg.dependencies ?? {}),
    ...Object.keys(pkg.devDependencies ?? {}),
    ...Object.keys(pkg.peerDependencies ?? {}),
  ];
}

describe('CodeMirror stays out of the web reader bundle', () => {
  it('the shared @mdreader/ui package declares no CodeMirror/Lezer dependency', () => {
    const leaked = allDeps(uiManifest).filter((dep) => EDITOR_DEP.test(dep));
    expect(leaked).toEqual([]);
  });

  it('the web app declares no CodeMirror/Lezer dependency and only reaches the workspace via @mdreader/ui', () => {
    expect(allDeps(webManifest).filter((dep) => EDITOR_DEP.test(dep))).toEqual([]);
    // Its only RUNTIME @mdreader dependency (what can reach the shipped bundle) is
    // the shared reader — there is no path to the desktop app package, where CM6
    // lives. (@mdreader/config is a build-time devDependency, not bundled.)
    const runtime = Object.keys(webManifest.dependencies ?? {}).filter((dep) =>
      dep.startsWith('@mdreader/'),
    );
    expect(runtime).toEqual(['@mdreader/ui']);
  });

  it('the desktop app package IS where CodeMirror lives (positive control)', () => {
    const deps = allDeps(appManifest);
    expect(deps.some((dep) => dep.startsWith('@codemirror/'))).toBe(true);
  });
});
