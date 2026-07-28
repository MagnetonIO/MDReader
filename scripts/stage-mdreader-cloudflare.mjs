import assert from 'node:assert/strict';
import {
  lstatSync,
  realpathSync,
  writeFileSync,
} from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { build } from 'esbuild';

import {
  assertMdreaderCloudflareDestinationPlan,
  assertMdreaderPinnedCopySupport,
  createMdreaderCloudflareOutputStage,
  discardMdreaderCloudflareOutputStage,
  publishMdreaderCloudflareOutput,
  readPinnedMdreaderCloudflareSource,
  snapshotMdreaderCloudflareOutputManifest,
  snapshotMdreaderCloudflareSourceFile,
  snapshotMdreaderCloudflareSourceTree,
  stageMdreaderCloudflareAssetPlan,
} from './mdreader-cloudflare-assets.mjs';

const ROOT = dirname(dirname(fileURLToPath(import.meta.url)));
const APP = realpathSync.native(resolve(ROOT, process.argv[2] ?? 'apps/web'));
const NEXT = join(APP, '.next');
const SERVER_APP = join(NEXT, 'server', 'app');
const OPEN_NEXT = join(APP, '.open-next');
const PUBLIC = join(APP, 'public');

assertMdreaderPinnedCopySupport();

function readJson(relativePath) {
  const source = snapshotMdreaderCloudflareSourceFile(
    NEXT,
    relativePath,
    `.next/${relativePath}`,
  );
  return JSON.parse(readPinnedMdreaderCloudflareSource(source).toString('utf8'));
}

function routeFile(route, suffix) {
  assert.match(route, /^\/[^\\]*$/, `unsafe prerender route ${route}`);
  assert.equal(route.includes('..'), false, `unsafe prerender route ${route}`);
  const base = route === '/' ? 'index' : route.slice(1);
  return `${base}${suffix}`;
}

function sourceExists(path) {
  try {
    lstatSync(path);
    return true;
  } catch (error) {
    if (error.code === 'ENOENT') return false;
    throw error;
  }
}

function addFile(plan, root, sourceRelative, sourceName, destination) {
  plan.push({
    destination,
    source: snapshotMdreaderCloudflareSourceFile(
      root,
      sourceRelative,
      sourceName,
    ),
  });
}

function addTree(plan, root, sourceName, destinationRoot) {
  for (const source of snapshotMdreaderCloudflareSourceTree(root, sourceName)) {
    plan.push({
      destination: `${destinationRoot}/${source.relativePath}`,
      source,
    });
  }
}

function addRequiredRoute(
  plan,
  sourceRelative,
  destinationRelative = sourceRelative,
) {
  addFile(
    plan,
    SERVER_APP,
    sourceRelative,
    `.next/server/app/${sourceRelative}`,
    destinationRelative,
  );
}

const routesManifest = readJson('routes-manifest.json');
const dynamicPages = routesManifest.dynamicRoutes.map((route) => route.page).sort();
assert.deepEqual(
  dynamicPages,
  ['/api/catdb/[...path]', '/docs/[slug]'],
  'Cloudflare staging refuses a new runtime route; add an explicit lightweight Worker handler or keep it prerendered',
);
assert.deepEqual(routesManifest.rewrites, {
  beforeFiles: [],
  afterFiles: [],
  fallback: [],
});

const prerenderManifest = readJson('prerender-manifest.json');
const assetPlan = [];
addFile(
  assetPlan,
  NEXT,
  'BUILD_ID',
  '.next/BUILD_ID',
  'BUILD_ID',
);
addTree(
  assetPlan,
  join(NEXT, 'static'),
  '.next/static',
  '_next/static',
);
if (sourceExists(PUBLIC)) {
  for (const source of snapshotMdreaderCloudflareSourceTree(PUBLIC, 'public')) {
    assetPlan.push({ destination: source.relativePath, source });
  }
}
for (const [route, metadata] of Object.entries(prerenderManifest.routes)) {
  assert.equal(
    metadata.initialRevalidateSeconds,
    false,
    `Cloudflare static route ${route} unexpectedly requires revalidation`,
  );
  if (metadata.dataRoute) {
    const html = routeFile(route, '.html');
    addRequiredRoute(
      assetPlan,
      html,
    );
    const dataRoute = metadata.dataRoute.slice(1);
    addRequiredRoute(
      assetPlan,
      dataRoute,
    );
    const segmentsRelative = routeFile(route, '.segments');
    const segments = join(SERVER_APP, segmentsRelative);
    if (sourceExists(segments)) {
      addTree(
        assetPlan,
        segments,
        `.next/server/app/${segmentsRelative}`,
        segmentsRelative,
      );
    }
  } else {
    addRequiredRoute(
      assetPlan,
      routeFile(route, '.body'),
      route.slice(1),
    );
  }
}
addRequiredRoute(
  assetPlan,
  '_not-found.html',
  '404.html',
);

// The complete mapping must be unambiguous before .open-next is touched.
assertMdreaderCloudflareDestinationPlan(assetPlan);
const outputStage = createMdreaderCloudflareOutputStage(OPEN_NEXT);
let assetSet;
let published = false;
try {
  assetSet = stageMdreaderCloudflareAssetPlan(assetPlan, outputStage);
  const workerBuild = await build({
    entryPoints: [join(APP, 'cloudflare-worker.ts')],
    outfile: join(outputStage.stagingRoot, 'worker.js'),
    bundle: true,
    format: 'esm',
    platform: 'browser',
    target: 'es2022',
    minify: true,
    legalComments: 'none',
    logLevel: 'warning',
    metafile: true,
  });
  writeFileSync(
    join(outputStage.stagingRoot, 'worker.meta.json'),
    `${JSON.stringify(workerBuild.metafile, null, 2)}\n`,
  );
  const outputManifest =
    snapshotMdreaderCloudflareOutputManifest(outputStage);
  publishMdreaderCloudflareOutput(outputStage, outputManifest);
  published = true;
} finally {
  if (!published) discardMdreaderCloudflareOutputStage(outputStage);
}

console.log(
  `Staged ${Object.keys(prerenderManifest.routes).length} MDReader prerenders, ${assetSet.fileCount} bounded assets (${assetSet.totalBytes} bytes), and the bounded CatDB proxy Worker`,
);
