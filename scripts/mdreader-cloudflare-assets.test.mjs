import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  rmSync,
  symlinkSync,
  truncateSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

const ROOT = dirname(dirname(fileURLToPath(import.meta.url)));
const BUNDLE_GATE = join(ROOT, 'scripts/assert-mdreader-cloudflare-bundle.mjs');
const STAGE = join(ROOT, 'scripts/stage-mdreader-cloudflare.mjs');
const MIB = 1024 * 1024;

function write(path, contents = '') {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, contents);
}

function run(script, app) {
  return spawnSync(process.execPath, [script, app], {
    cwd: ROOT,
    encoding: 'utf8',
    timeout: 60_000,
  });
}

function output(result) {
  return `${result.stdout ?? ''}\n${result.stderr ?? ''}`;
}

function assertRejected(result, pattern) {
  assert.notEqual(result.status, 0, `gate unexpectedly passed:\n${output(result)}`);
  assert.match(output(result), pattern);
}

function bundleFixture() {
  const directory = mkdtempSync(join(tmpdir(), 'mdreader-assets-bundle-'));
  const app = join(directory, 'app');
  const openNext = join(app, '.open-next');
  const assets = join(openNext, 'assets');

  write(join(openNext, 'worker.js'), 'export default {};\n');
  write(join(openNext, 'worker.meta.json'), '{"inputs":{}}\n');
  write(
    join(assets, 'docs/markdown-support.html'),
    '<main class="reader-surface" data-line-start="1"></main>',
  );
  write(join(assets, 'app.html'), '<script>fetch("/api/catdb")</script>');
  write(join(assets, 'index.html'), '<h1>MDReader</h1>');
  write(join(assets, 'robots.txt'), 'User-agent: *\n');
  write(join(assets, 'sitemap.xml'), '<urlset></urlset>\n');

  const wrangler = join(app, 'node_modules/.bin/wrangler');
  write(
    wrangler,
    `#!/usr/bin/env node
const fs = require('node:fs');
const path = require('node:path');
const args = process.argv.slice(2);
const metafile = args[args.indexOf('--metafile') + 1];
fs.mkdirSync(path.dirname(metafile), { recursive: true });
fs.writeFileSync(metafile, JSON.stringify({ inputs: {} }));
process.stdout.write('Total Upload: 1.00 KiB / gzip: 1.00 KiB\\n');
`,
  );
  chmodSync(wrangler, 0o755);

  return { app, assets, directory };
}

function stageFixture({ publicSymlink = false, staticSymlink = false } = {}) {
  const directory = mkdtempSync(join(tmpdir(), 'mdreader-assets-stage-'));
  const app = join(directory, 'app');
  const next = join(app, '.next');

  write(
    join(next, 'routes-manifest.json'),
    JSON.stringify({
      dynamicRoutes: [
        { page: '/api/catdb/[...path]' },
        { page: '/docs/[slug]' },
      ],
      rewrites: { beforeFiles: [], afterFiles: [], fallback: [] },
    }),
  );
  write(
    join(next, 'prerender-manifest.json'),
    JSON.stringify({
      routes: {
        '/': { initialRevalidateSeconds: false, dataRoute: '/index.rsc' },
      },
    }),
  );
  write(join(next, 'BUILD_ID'), 'test-build\n');
  write(join(next, 'server/app/index.html'), '<h1>MDReader</h1>');
  write(join(next, 'server/app/index.rsc'), '0:{"tree":{}}\n');
  write(join(next, 'server/app/_not-found.html'), '<h1>Not found</h1>');
  write(join(next, 'static/chunks/real.js'), 'export {};\n');
  if (staticSymlink) {
    symlinkSync('real.js', join(next, 'static/chunks/linked.js'));
  }
  if (publicSymlink) {
    const externalPublic = join(directory, 'external-public');
    write(join(externalPublic, 'leak.txt'), 'must not be staged\n');
    symlinkSync(externalPublic, join(app, 'public'), 'dir');
  }
  write(
    join(app, 'cloudflare-worker.ts'),
    "export default { fetch() { return new Response('ok'); } };\n",
  );

  return { app, directory };
}

test('staging rejects a symlink before publishing the asset set', () => {
  const fixture = stageFixture({ staticSymlink: true });
  try {
    assertRejected(run(STAGE, fixture.app), /symbolic link.*linked\.js/is);
  } finally {
    rmSync(fixture.directory, { recursive: true, force: true });
  }
});

test('staging rejects a symlinked source root before copying its bytes', () => {
  const fixture = stageFixture({ publicSymlink: true });
  try {
    const result = run(STAGE, fixture.app);
    assertRejected(result, /symbolic link.*public/is);
    assert.equal(
      existsSync(join(fixture.app, '.open-next/assets/leak.txt')),
      false,
      'staging copied bytes through the symlinked public root',
    );
  } finally {
    rmSync(fixture.directory, { recursive: true, force: true });
  }
});

test('bundle gate rejects a symlink anywhere in the exact staged set', () => {
  const fixture = bundleFixture();
  try {
    symlinkSync('index.html', join(fixture.assets, 'linked-index.html'));
    assertRejected(run(BUNDLE_GATE, fixture.app), /symbolic link.*linked-index\.html/is);
  } finally {
    rmSync(fixture.directory, { recursive: true, force: true });
  }
});

test('bundle gate rejects more than the Cloudflare Free asset count', () => {
  const fixture = bundleFixture();
  try {
    for (let index = 0; index < 19_996; index += 1) {
      write(join(fixture.assets, 'bulk', String(index)));
    }
    assertRejected(run(BUNDLE_GATE, fixture.app), /20,001.*20,000|20001.*20000/s);
  } finally {
    rmSync(fixture.directory, { recursive: true, force: true });
  }
});

test('bundle gate rejects an asset above Cloudflare per-file bytes', () => {
  const fixture = bundleFixture();
  try {
    const asset = join(fixture.assets, 'oversize.bin');
    write(asset);
    truncateSync(asset, 25 * MIB + 1);
    assertRejected(run(BUNDLE_GATE, fixture.app), /oversize\.bin.*26,214,401.*26,214,400/is);
  } finally {
    rmSync(fixture.directory, { recursive: true, force: true });
  }
});

test('bundle gate rejects a staged set above the project total-byte budget', () => {
  const fixture = bundleFixture();
  try {
    for (const name of ['one.bin', 'two.bin', 'three.bin']) {
      const asset = join(fixture.assets, name);
      write(asset);
      truncateSync(asset, 22 * MIB);
    }
    assertRejected(run(BUNDLE_GATE, fixture.app), /total.*67,108,864/is);
  } finally {
    rmSync(fixture.directory, { recursive: true, force: true });
  }
});
