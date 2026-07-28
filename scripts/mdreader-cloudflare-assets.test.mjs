import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import {
  chmodSync,
  existsSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  realpathSync,
  readdirSync,
  rmSync,
  symlinkSync,
  truncateSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import test from 'node:test';

const ROOT = dirname(dirname(fileURLToPath(import.meta.url)));
const BUNDLE_GATE = join(ROOT, 'scripts/assert-mdreader-cloudflare-bundle.mjs');
const STAGE = join(ROOT, 'scripts/stage-mdreader-cloudflare.mjs');
const MIB = 1024 * 1024;

function write(path, contents = '') {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, contents);
}

function run(script, app, environment = {}) {
  return spawnSync(process.execPath, [script, app], {
    cwd: ROOT,
    encoding: 'utf8',
    env: { ...process.env, ...environment },
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

function stageFixture({
  publicFiles = {},
  publicSymlink = false,
  staticSymlink = false,
} = {}) {
  const directory = realpathSync.native(
    mkdtempSync(join(tmpdir(), 'mdreader-assets-stage-')),
  );
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
  } else {
    for (const [name, contents] of Object.entries(publicFiles)) {
      write(join(app, 'public', name), contents);
    }
  }
  write(
    join(app, 'cloudflare-worker.ts'),
    "export default { fetch() { return new Response('ok'); } };\n",
  );

  return { app, directory, next };
}

function sourceSwapSeam(fixture, options) {
  const hook = join(fixture.directory, `swap-${options.kind}.mjs`);
  const marker = join(fixture.directory, `swap-${options.kind}.ran`);
  write(
    hook,
    `import {
  copyFileSync,
  renameSync,
  symlinkSync,
  writeFileSync,
} from 'node:fs';

let swapped = false;
globalThis[Symbol.for('mdreader.cloudflare.before-source-open')] = ({ source }) => {
  if (swapped || source !== process.env.MDREADER_TEST_SWAP_SOURCE) return;
  swapped = true;
  writeFileSync(process.env.MDREADER_TEST_SWAP_MARKER, 'ran\\n');
  if (process.env.MDREADER_TEST_SWAP_KIND === 'file') {
    renameSync(source, source + '.validated');
    copyFileSync(process.env.MDREADER_TEST_SWAP_EXTERNAL, source);
    return;
  }
  const ancestor = process.env.MDREADER_TEST_SWAP_ANCESTOR;
  renameSync(ancestor, ancestor + '.validated');
  symlinkSync(process.env.MDREADER_TEST_SWAP_EXTERNAL, ancestor, 'dir');
};
`,
  );
  const existingNodeOptions = process.env.NODE_OPTIONS?.trim();
  return {
    environment: {
      MDREADER_TEST_SWAP_ANCESTOR: options.ancestor ?? '',
      MDREADER_TEST_SWAP_EXTERNAL: options.external,
      MDREADER_TEST_SWAP_KIND: options.kind,
      MDREADER_TEST_SWAP_MARKER: marker,
      MDREADER_TEST_SWAP_SOURCE: options.source,
      NODE_OPTIONS: [
        existingNodeOptions,
        `--import=${pathToFileURL(hook).href}`,
      ]
        .filter(Boolean)
        .join(' '),
    },
    marker,
  };
}

function assertExternalBytesAbsent(app, destination, sentinel) {
  const openNext = join(app, '.open-next');
  if (!existsSync(openNext)) return;
  const sentinelBytes = Buffer.from(sentinel);
  function walk(current) {
    for (const entry of readdirSync(current)) {
      const path = join(current, entry);
      const metadata = lstatSync(path);
      assert.equal(
        metadata.isSymbolicLink(),
        false,
        `staging left a symbolic link after rejecting ${destination}`,
      );
      if (metadata.isDirectory()) {
        walk(path);
        continue;
      }
      assert.equal(
        readFileSync(path).includes(sentinelBytes),
        false,
        `external bytes reached .open-next while staging ${destination}`,
      );
    }
  }
  walk(openNext);
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

test('staging rejects a validated leaf swapped exactly before source open', () => {
  const fixture = stageFixture();
  const sentinel = 'EXTERNAL-LEAF-BYTES';
  const external = join(fixture.directory, 'external-build-id');
  write(external, sentinel);
  const seam = sourceSwapSeam(fixture, {
    external,
    kind: 'file',
    source: join(fixture.next, 'BUILD_ID'),
  });
  try {
    const result = run(STAGE, fixture.app, seam.environment);
    assertRejected(result, /source.*(?:identity|changed)|identity.*source/is);
    assert.equal(
      existsSync(seam.marker),
      true,
      `source-open seam did not run:\n${output(result)}`,
    );
    assertExternalBytesAbsent(fixture.app, 'BUILD_ID', sentinel);
  } finally {
    rmSync(fixture.directory, { recursive: true, force: true });
  }
});

test('staging rejects an ancestor tree swapped exactly before source open', () => {
  const fixture = stageFixture();
  const sentinel = 'EXTERNAL-ANCESTOR-BYTES';
  const external = join(fixture.directory, 'external-static');
  write(join(external, 'chunks/real.js'), sentinel);
  const staticRoot = join(fixture.next, 'static');
  const seam = sourceSwapSeam(fixture, {
    ancestor: staticRoot,
    external,
    kind: 'tree',
    source: join(staticRoot, 'chunks/real.js'),
  });
  try {
    const result = run(STAGE, fixture.app, seam.environment);
    assertRejected(result, /source.*(?:ancestor|identity|changed)|symbolic link/is);
    assert.equal(
      existsSync(seam.marker),
      true,
      `source-open seam did not run:\n${output(result)}`,
    );
    assertExternalBytesAbsent(
      fixture.app,
      '_next/static/chunks/real.js',
      sentinel,
    );
  } finally {
    rmSync(fixture.directory, { recursive: true, force: true });
  }
});

test('staging rejects colliding destinations before writing either source', () => {
  const fixture = stageFixture({
    publicFiles: { BUILD_ID: 'PUBLIC-COLLISION\n' },
  });
  try {
    const result = run(STAGE, fixture.app);
    assertRejected(result, /destination.*BUILD_ID.*collid/is);
    assert.equal(
      existsSync(join(fixture.app, '.open-next')),
      false,
      'staging created output before rejecting the destination collision',
    );
  } finally {
    rmSync(fixture.directory, { recursive: true, force: true });
  }
});

test('staging rejects file and directory destination overlap before writing', () => {
  const fixture = stageFixture({
    publicFiles: { _next: 'PUBLIC-PREFIX-COLLISION\n' },
  });
  try {
    const result = run(STAGE, fixture.app);
    assertRejected(result, /destination.*_next.*collid/is);
    assert.equal(
      existsSync(join(fixture.app, '.open-next')),
      false,
      'staging created output before rejecting the destination overlap',
    );
  } finally {
    rmSync(fixture.directory, { recursive: true, force: true });
  }
});

test('staging rejects a symlinked output root without touching its target', () => {
  const fixture = stageFixture();
  const external = join(fixture.directory, 'external-output');
  const sentinel = join(external, 'keep.txt');
  write(sentinel, 'KEEP-EXTERNAL-OUTPUT\n');
  symlinkSync(external, join(fixture.app, '.open-next'), 'dir');
  try {
    const result = run(STAGE, fixture.app);
    assertRejected(result, /destination.*symbolic link.*\.open-next/is);
    assert.equal(
      readFileSync(sentinel, 'utf8'),
      'KEEP-EXTERNAL-OUTPUT\n',
      'staging touched the symlinked output target',
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
