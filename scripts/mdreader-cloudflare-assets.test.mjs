import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import {
  chmodSync,
  existsSync,
  linkSync,
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

import {
  assertMdreaderCloudflareDestinationPlan,
} from './mdreader-cloudflare-assets.mjs';

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
globalThis[Symbol.for(process.env.MDREADER_TEST_SWAP_SEAM)] = ({ source }) => {
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
      MDREADER_TEST_SWAP_SEAM:
        options.seam ?? 'mdreader.cloudflare.before-source-open',
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

function prepublicationMutationSeam(fixture) {
  const hook = join(fixture.directory, 'mutate-staged-output.mjs');
  const marker = join(fixture.directory, 'mutate-staged-output.ran');
  write(
    hook,
    `import { writeFileSync } from 'node:fs';
import { join } from 'node:path';

globalThis[Symbol.for('mdreader.cloudflare.before-publication')] = ({ stagingRoot }) => {
  writeFileSync(process.env.MDREADER_TEST_MUTATION_MARKER, 'ran\\n');
  writeFileSync(join(stagingRoot, 'assets', 'BUILD_ID'), 'evil-build\\n');
};
`,
  );
  const existingNodeOptions = process.env.NODE_OPTIONS?.trim();
  return {
    environment: {
      MDREADER_TEST_MUTATION_MARKER: marker,
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

function publicationFaultSeam(fixture, rules) {
  const hook = join(
    fixture.directory,
    `publication-fault-${Math.random().toString(36).slice(2)}.mjs`,
  );
  write(
    hook,
    `import {
  mkdirSync,
  renameSync,
  writeFileSync,
} from 'node:fs';

const rules = JSON.parse(process.env.MDREADER_TEST_PUBLICATION_RULES);
const consumed = new Set();

function applyRule(scope, detail) {
  const rule = rules.find((candidate, index) =>
    candidate.scope === scope
      && candidate.step === detail.step
      && (!candidate.once || !consumed.has(index)));
  if (!rule) return;
  consumed.add(rules.indexOf(rule));
  writeFileSync(
    process.env.MDREADER_TEST_PUBLICATION_MARKER,
    JSON.stringify({ scope, step: detail.step }) + '\\n',
    { flag: 'a' },
  );
  if (rule.action === 'throw') {
    throw new Error(rule.message ?? ('injected ' + scope + ' failure at ' + detail.step));
  }
  if (rule.action === 'exit') {
    process.exit(Number(rule.code ?? 86));
  }
  if (rule.action === 'rebind-stage') {
    renameSync(detail.stagingRoot, detail.stagingRoot + '.owned');
    mkdirSync(detail.stagingRoot, { mode: 0o700 });
    writeFileSync(detail.stagingRoot + '/decoy.txt', 'DO-NOT-DELETE\\n');
    return;
  }
  if (rule.action === 'rebind-parent') {
    renameSync(detail.parent, detail.parent + '.owned');
    mkdirSync(detail.parent, { mode: 0o700 });
    mkdirSync(detail.stagingRoot, { mode: 0o700 });
    writeFileSync(detail.stagingRoot + '/decoy.txt', 'DO-NOT-DELETE\\n');
    return;
  }
  throw new Error('unknown publication test action ' + rule.action);
}

globalThis[Symbol.for('mdreader.cloudflare.publication-step')] =
  (detail) => applyRule('publication', detail);
globalThis[Symbol.for('mdreader.cloudflare.cleanup-step')] =
  (detail) => applyRule('cleanup', detail);
`,
  );
  const marker = join(fixture.directory, 'publication-faults.ran');
  const existingNodeOptions = process.env.NODE_OPTIONS?.trim();
  return {
    environment: {
      MDREADER_TEST_PUBLICATION_MARKER: marker,
      MDREADER_TEST_PUBLICATION_RULES: JSON.stringify(rules),
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

function snapshotTree(root) {
  const entries = [];
  function walk(current, relativeDirectory = '') {
    for (const entry of readdirSync(current).sort()) {
      const path = join(current, entry);
      const relativePath = relativeDirectory
        ? `${relativeDirectory}/${entry}`
        : entry;
      const metadata = lstatSync(path);
      assert.equal(metadata.isSymbolicLink(), false);
      if (metadata.isDirectory()) {
        entries.push({
          kind: 'directory',
          mode: metadata.mode & 0o777,
          path: relativePath,
        });
        walk(path, relativePath);
        continue;
      }
      assert.equal(metadata.isFile(), true);
      entries.push({
        bytes: readFileSync(path).toString('base64'),
        kind: 'file',
        mode: metadata.mode & 0o777,
        path: relativePath,
      });
    }
  }
  walk(root);
  return entries;
}

function seedPublishedOutput(fixture) {
  const output = join(fixture.app, '.open-next');
  write(join(output, 'worker.js'), 'export default { old: true };\n');
  write(join(output, 'worker.meta.json'), '{"old":true}\n');
  write(join(output, 'assets/old.txt'), 'OLD-PUBLISHED-BYTES\n');
  return { output, snapshot: snapshotTree(output) };
}

function privateOutputEntries(output) {
  return readdirSync(dirname(output))
    .filter((entry) =>
      entry.startsWith('.open-next-stage-')
        || entry.startsWith('.open-next-recovery')
        || entry.includes('.owned'))
    .sort();
}

function findPriorOutput(output) {
  const recovery = join(dirname(output), '.open-next-recovery');
  const previous = join(recovery, 'previous');
  return existsSync(previous) ? previous : null;
}

function assertPublishedOutputUnchanged(published) {
  assert.equal(
    existsSync(published.output),
    true,
    'staging removed the previously published output',
  );
  assert.deepEqual(
    snapshotTree(published.output),
    published.snapshot,
    'staging changed the previously published output',
  );
  const siblings = readdirSync(dirname(published.output))
    .filter((entry) =>
      entry.startsWith('.open-next-stage-')
        || entry.startsWith('.open-next-backup-')
        || entry.startsWith('.open-next-recovery'));
  assert.deepEqual(
    siblings,
    [],
    `staging leaked private output trees: ${siblings.join(', ')}`,
  );
}

function assertPublishedOutputBytesUnchanged(published) {
  assert.equal(
    existsSync(published.output),
    true,
    'staging removed the previously published output',
  );
  assert.deepEqual(
    snapshotTree(published.output),
    published.snapshot,
    'staging changed the previously published output',
  );
}

function destinationPlan(...destinations) {
  return destinations.map((destination) => ({
    destination,
    source: { identity: { size: 1n } },
  }));
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

test('staging rejects an ancestor swapped after its final check but before source open', () => {
  const fixture = stageFixture();
  const sentinel = 'EXTERNAL-POST-CHECK-BYTES';
  const external = join(fixture.directory, 'external-static-post-check');
  write(join(external, 'chunks/real.js'), sentinel);
  const staticRoot = join(fixture.next, 'static');
  const seam = sourceSwapSeam(fixture, {
    ancestor: staticRoot,
    external,
    kind: 'post-check-tree',
    seam: 'mdreader.cloudflare.after-source-ancestor-check',
    source: join(staticRoot, 'chunks/real.js'),
  });
  const published = seedPublishedOutput(fixture);
  try {
    const result = run(STAGE, fixture.app, seam.environment);
    assertRejected(result, /source.*(?:ancestor|identity|changed)|symbolic link/is);
    assert.equal(
      existsSync(seam.marker),
      true,
      `post-check source-open seam did not run:\n${output(result)}`,
    );
    assertPublishedOutputUnchanged(published);
    assertExternalBytesAbsent(
      fixture.app,
      '_next/static/chunks/real.js',
      sentinel,
    );
  } finally {
    rmSync(fixture.directory, { recursive: true, force: true });
  }
});

test('staging rejects a multiply-linked source file', () => {
  const fixture = stageFixture();
  const externalLink = join(fixture.directory, 'external-build-id-link');
  linkSync(join(fixture.next, 'BUILD_ID'), externalLink);
  try {
    assertRejected(
      run(STAGE, fixture.app),
      /source.*(?:multiple|link)|link count/is,
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

test('destination plan rejects case-only collisions within and across asset families', () => {
  for (const destinations of [
    ['README.txt', 'readme.txt'],
    ['_NEXT/static/chunks/app.js', '_next/static/chunks/app.js'],
  ]) {
    assert.throws(
      () => assertMdreaderCloudflareDestinationPlan(destinationPlan(...destinations)),
      /destination.*collid/is,
    );
  }
});

test('destination plan rejects Unicode-canonically-equivalent collisions', () => {
  for (const destinations of [
    ['café.txt', 'cafe\u0301.txt'],
    ['public/café.txt', 'public/cafe\u0301.txt'],
  ]) {
    assert.throws(
      () => assertMdreaderCloudflareDestinationPlan(destinationPlan(...destinations)),
      /destination.*collid/is,
    );
  }
});

test('destination plan rejects normalized file and directory prefix conflicts', () => {
  for (const destinations of [
    ['Docs', 'docs/page.html'],
    ['café', 'cafe\u0301/page.html'],
  ]) {
    assert.throws(
      () => assertMdreaderCloudflareDestinationPlan(destinationPlan(...destinations)),
      /destination.*collid/is,
    );
  }
});

test('destination plan rejects compatibility-case collisions used by supported filesystems', () => {
  for (const destinations of [
    ['Straße.txt', 'STRASSE.txt'],
    ['greek/σ.txt', 'greek/ς.txt'],
    ['historic/ſ.txt', 'historic/s.txt'],
  ]) {
    assert.throws(
      () => assertMdreaderCloudflareDestinationPlan(destinationPlan(...destinations)),
      /destination.*collid/is,
    );
  }
});

test('source-copy failure preserves the previously published Worker tree', () => {
  const fixture = stageFixture();
  const sentinel = 'EXTERNAL-SOURCE-FAILURE';
  const external = join(fixture.directory, 'external-source-failure');
  write(external, sentinel);
  const seam = sourceSwapSeam(fixture, {
    external,
    kind: 'file',
    source: join(fixture.next, 'BUILD_ID'),
  });
  const published = seedPublishedOutput(fixture);
  try {
    const result = run(STAGE, fixture.app, seam.environment);
    assertRejected(result, /source.*(?:identity|changed)|identity.*source/is);
    assertPublishedOutputUnchanged(published);
  } finally {
    rmSync(fixture.directory, { recursive: true, force: true });
  }
});

test('Worker compilation failure preserves the previously published Worker tree', () => {
  const fixture = stageFixture();
  write(join(fixture.app, 'cloudflare-worker.ts'), 'export default { broken: ; };\n');
  const published = seedPublishedOutput(fixture);
  try {
    assertRejected(run(STAGE, fixture.app), /error|unexpected/i);
    assertPublishedOutputUnchanged(published);
  } finally {
    rmSync(fixture.directory, { recursive: true, force: true });
  }
});

test('prepublication staged-byte mutation preserves the published Worker tree', () => {
  const fixture = stageFixture();
  const seam = prepublicationMutationSeam(fixture);
  const published = seedPublishedOutput(fixture);
  try {
    const result = run(STAGE, fixture.app, seam.environment);
    assertRejected(result, /staged.*(?:digest|manifest|changed)|manifest.*differ/is);
    assert.equal(
      existsSync(seam.marker),
      true,
      `prepublication seam did not run:\n${output(result)}`,
    );
    assertPublishedOutputUnchanged(published);
  } finally {
    rmSync(fixture.directory, { recursive: true, force: true });
  }
});

test('first publication rename failure preserves the prior output and cleans the stage', () => {
  const fixture = stageFixture();
  const published = seedPublishedOutput(fixture);
  const seam = publicationFaultSeam(fixture, [{
    action: 'throw',
    scope: 'publication',
    step: 'before-backup-rename',
  }]);
  try {
    const result = run(STAGE, fixture.app, seam.environment);
    assertRejected(result, /before-backup-rename|publication/i);
    assertPublishedOutputUnchanged(published);
  } finally {
    rmSync(fixture.directory, { recursive: true, force: true });
  }
});

test('second publication rename failure restores the prior output', () => {
  const fixture = stageFixture();
  const published = seedPublishedOutput(fixture);
  const seam = publicationFaultSeam(fixture, [{
    action: 'throw',
    scope: 'publication',
    step: 'before-replacement-rename',
  }]);
  try {
    const result = run(STAGE, fixture.app, seam.environment);
    assertRejected(result, /before-replacement-rename|publication/i);
    assertPublishedOutputUnchanged(published);
  } finally {
    rmSync(fixture.directory, { recursive: true, force: true });
  }
});

test('post-rename identity failure rolls both moves back without losing either tree', () => {
  const fixture = stageFixture();
  const published = seedPublishedOutput(fixture);
  const seam = publicationFaultSeam(fixture, [{
    action: 'throw',
    scope: 'publication',
    step: 'after-replacement-identity',
  }]);
  try {
    const result = run(STAGE, fixture.app, seam.environment);
    assertRejected(result, /after-replacement-identity|publication/i);
    assertPublishedOutputUnchanged(published);
  } finally {
    rmSync(fixture.directory, { recursive: true, force: true });
  }
});

test('failed replacement rollback preserves the new output and recoverable prior tree', () => {
  const fixture = stageFixture();
  const published = seedPublishedOutput(fixture);
  const seam = publicationFaultSeam(fixture, [
    {
      action: 'throw',
      scope: 'publication',
      step: 'after-replacement-identity',
    },
    {
      action: 'throw',
      scope: 'publication',
      step: 'before-rollback-replacement',
    },
  ]);
  try {
    const result = run(STAGE, fixture.app, seam.environment);
    assertRejected(result, /AggregateError|rollback was incomplete/i);
    assert.equal(existsSync(published.output), true, 'replacement disappeared');
    const prior = findPriorOutput(published.output);
    assert.notEqual(prior, null, 'prior output was not retained for recovery');
    assert.deepEqual(snapshotTree(prior), published.snapshot);
  } finally {
    rmSync(fixture.directory, { recursive: true, force: true });
  }
});

test('failed prior restore never deletes the only prior tree', () => {
  const fixture = stageFixture();
  const published = seedPublishedOutput(fixture);
  const seam = publicationFaultSeam(fixture, [
    {
      action: 'throw',
      scope: 'publication',
      step: 'before-replacement-rename',
    },
    {
      action: 'throw',
      scope: 'publication',
      step: 'before-rollback-restore',
    },
  ]);
  try {
    const result = run(STAGE, fixture.app, seam.environment);
    assertRejected(result, /AggregateError|rollback was incomplete/i);
    assert.equal(
      existsSync(published.output),
      false,
      'failed restore unexpectedly installed another tree',
    );
    const prior = findPriorOutput(published.output);
    assert.notEqual(prior, null, 'failed restore deleted the prior release');
    assert.deepEqual(snapshotTree(prior), published.snapshot);
    assert.ok(
      privateOutputEntries(published.output).some((entry) =>
        entry.startsWith('.open-next-stage-')),
      'failed restore deleted the complete replacement stage',
    );
  } finally {
    rmSync(fixture.directory, { recursive: true, force: true });
  }
});

test('backup cleanup failure preserves the published replacement and prior tree', () => {
  const fixture = stageFixture();
  const published = seedPublishedOutput(fixture);
  const seam = publicationFaultSeam(fixture, [{
    action: 'throw',
    scope: 'publication',
    step: 'before-recovery-cleanup',
  }]);
  try {
    const result = run(STAGE, fixture.app, seam.environment);
    assertRejected(result, /AggregateError|recovery cleanup/i);
    assert.equal(existsSync(published.output), true, 'replacement was removed');
    assert.notDeepEqual(snapshotTree(published.output), published.snapshot);
    const prior = findPriorOutput(published.output);
    assert.notEqual(prior, null, 'cleanup failure deleted the recoverable prior');
    assert.deepEqual(snapshotTree(prior), published.snapshot);
  } finally {
    rmSync(fixture.directory, { recursive: true, force: true });
  }
});

test('finally cleanup refuses a rebound staging path and preserves the primary error', () => {
  const fixture = stageFixture();
  write(join(fixture.app, 'cloudflare-worker.ts'), 'export default { broken: ; };\n');
  const published = seedPublishedOutput(fixture);
  const seam = publicationFaultSeam(fixture, [{
    action: 'rebind-stage',
    scope: 'cleanup',
    step: 'before-stage-discard',
  }]);
  try {
    const result = run(STAGE, fixture.app, seam.environment);
    assertRejected(result, /AggregateError/i);
    assert.match(output(result), /unexpected|error/i);
    assert.match(output(result), /identity|rebound|cleanup/i);
    assertPublishedOutputBytesUnchanged(published);
    const siblings = privateOutputEntries(published.output);
    assert.ok(siblings.some((entry) => entry.endsWith('.owned')));
    assert.ok(siblings.some((entry) => entry.startsWith('.open-next-stage-')));
  } finally {
    rmSync(fixture.directory, { recursive: true, force: true });
  }
});

test('finally cleanup failure is aggregated without replacing the primary error', () => {
  const fixture = stageFixture();
  write(join(fixture.app, 'cloudflare-worker.ts'), 'export default { broken: ; };\n');
  const published = seedPublishedOutput(fixture);
  const seam = publicationFaultSeam(fixture, [{
    action: 'throw',
    scope: 'cleanup',
    step: 'before-stage-discard',
  }]);
  try {
    const result = run(STAGE, fixture.app, seam.environment);
    assertRejected(result, /AggregateError/i);
    assert.match(output(result), /unexpected|error/i);
    assert.match(output(result), /injected cleanup failure/i);
    assert.deepEqual(snapshotTree(published.output), published.snapshot);
    assert.ok(
      privateOutputEntries(published.output).some((entry) =>
        entry.startsWith('.open-next-stage-')),
      'failed cleanup deleted the private stage',
    );
  } finally {
    rmSync(fixture.directory, { recursive: true, force: true });
  }
});

test('finally cleanup refuses a rebound staging parent before recursive deletion', () => {
  const fixture = stageFixture();
  write(join(fixture.app, 'cloudflare-worker.ts'), 'export default { broken: ; }\n');
  const published = seedPublishedOutput(fixture);
  const seam = publicationFaultSeam(fixture, [{
    action: 'rebind-parent',
    scope: 'cleanup',
    step: 'before-stage-discard',
  }]);
  try {
    const result = run(STAGE, fixture.app, seam.environment);
    assertRejected(result, /AggregateError/i);
    assert.match(output(result), /unexpected|error/i);
    assert.match(output(result), /identity|cleanup parent/i);
    const originalApp = fixture.app + '.owned';
    const originalOutput = join(originalApp, '.open-next');
    assert.equal(existsSync(originalOutput), true);
    assert.deepEqual(snapshotTree(originalOutput), published.snapshot);
    assert.ok(
      readdirSync(fixture.app).some((entry) =>
        entry.startsWith('.open-next-stage-')),
      'cleanup deleted the rebound-parent decoy',
    );
  } finally {
    rmSync(fixture.directory, { recursive: true, force: true });
  }
});

test('successful replacement publishes one complete output with no recovery artifacts', () => {
  const fixture = stageFixture();
  const published = seedPublishedOutput(fixture);
  try {
    const result = run(STAGE, fixture.app);
    assert.equal(result.status, 0, output(result));
    assert.equal(existsSync(published.output), true);
    assert.notDeepEqual(snapshotTree(published.output), published.snapshot);
    assert.deepEqual(privateOutputEntries(published.output), []);
  } finally {
    rmSync(fixture.directory, { recursive: true, force: true });
  }
});

test('the next run restores the discoverable prior output after a crash boundary', () => {
  const fixture = stageFixture();
  const published = seedPublishedOutput(fixture);
  const crash = publicationFaultSeam(fixture, [{
    action: 'exit',
    code: 86,
    scope: 'publication',
    step: 'after-backup-identity',
  }]);
  try {
    const crashed = run(STAGE, fixture.app, crash.environment);
    assert.equal(crashed.status, 86, output(crashed));
    assert.equal(existsSync(published.output), false);
    const prior = findPriorOutput(published.output);
    assert.notEqual(prior, null, 'crash did not leave discoverable prior output');
    assert.deepEqual(snapshotTree(prior), published.snapshot);

    write(join(fixture.app, 'cloudflare-worker.ts'), 'export default { broken: ; };\n');
    const recoveryRun = run(STAGE, fixture.app);
    assertRejected(recoveryRun, /unexpected|error/i);
    assertPublishedOutputBytesUnchanged(published);
    assert.equal(findPriorOutput(published.output), null);
    assert.ok(
      privateOutputEntries(published.output).some((entry) =>
        entry.startsWith('.open-next-stage-')),
      'crash recovery discarded the preserved interrupted replacement',
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
