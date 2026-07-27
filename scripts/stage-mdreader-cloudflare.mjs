import assert from 'node:assert/strict';
import {
  copyFileSync,
  cpSync,
  existsSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { build } from 'esbuild';

import {
  assertMdreaderCloudflareAssets,
  assertMdreaderCloudflareSourceFile,
  assertMdreaderCloudflareSourceTree,
} from './mdreader-cloudflare-assets.mjs';

const ROOT = dirname(dirname(fileURLToPath(import.meta.url)));
const APP = resolve(ROOT, process.argv[2] ?? 'apps/web');
const NEXT = join(APP, '.next');
const SERVER_APP = join(NEXT, 'server', 'app');
const OPEN_NEXT = join(APP, '.open-next');
const ASSETS = join(OPEN_NEXT, 'assets');
const PUBLIC = join(APP, 'public');

function readJson(relativePath) {
  const source = assertMdreaderCloudflareSourceFile(
    join(NEXT, relativePath),
    `.next/${relativePath}`,
  );
  return JSON.parse(readFileSync(source, 'utf8'));
}

function routeFile(route, suffix) {
  assert.match(route, /^\/[^\\]*$/, `unsafe prerender route ${route}`);
  assert.equal(route.includes('..'), false, `unsafe prerender route ${route}`);
  const base = route === '/' ? 'index' : route.slice(1);
  return `${base}${suffix}`;
}

function copyRequired(sourceRelative, destinationRelative = sourceRelative) {
  const source = assertMdreaderCloudflareSourceFile(
    join(SERVER_APP, sourceRelative),
    `.next/server/app/${sourceRelative}`,
  );
  const destination = join(ASSETS, destinationRelative);
  mkdirSync(dirname(destination), { recursive: true });
  copyFileSync(source, destination);
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
assertMdreaderCloudflareSourceFile(
  join(NEXT, 'BUILD_ID'),
  '.next/BUILD_ID',
);
assertMdreaderCloudflareSourceTree(
  join(NEXT, 'static'),
  '.next/static',
);
if (existsSync(PUBLIC)) {
  assertMdreaderCloudflareSourceTree(PUBLIC, 'public');
}
for (const [route, metadata] of Object.entries(prerenderManifest.routes)) {
  if (metadata.dataRoute) {
    const html = routeFile(route, '.html');
    assertMdreaderCloudflareSourceFile(
      join(SERVER_APP, html),
      `.next/server/app/${html}`,
    );
    const dataRoute = metadata.dataRoute.slice(1);
    assertMdreaderCloudflareSourceFile(
      join(SERVER_APP, dataRoute),
      `.next/server/app/${dataRoute}`,
    );
    const segments = join(SERVER_APP, routeFile(route, '.segments'));
    if (existsSync(segments)) {
      assertMdreaderCloudflareSourceTree(
        segments,
        `.next/server/app/${routeFile(route, '.segments')}`,
      );
    }
  } else {
    const body = routeFile(route, '.body');
    assertMdreaderCloudflareSourceFile(
      join(SERVER_APP, body),
      `.next/server/app/${body}`,
    );
  }
}
assertMdreaderCloudflareSourceFile(
  join(SERVER_APP, '_not-found.html'),
  '.next/server/app/_not-found.html',
);

rmSync(OPEN_NEXT, { recursive: true, force: true });
mkdirSync(ASSETS, { recursive: true });
copyFileSync(
  assertMdreaderCloudflareSourceFile(
    join(NEXT, 'BUILD_ID'),
    '.next/BUILD_ID',
  ),
  join(ASSETS, 'BUILD_ID'),
);
assertMdreaderCloudflareSourceTree(join(NEXT, 'static'), '.next/static');
cpSync(join(NEXT, 'static'), join(ASSETS, '_next', 'static'), {
  recursive: true,
});
if (existsSync(PUBLIC)) {
  assertMdreaderCloudflareSourceTree(PUBLIC, 'public');
  for (const entry of readdirSync(PUBLIC)) {
    cpSync(join(PUBLIC, entry), join(ASSETS, entry), { recursive: true });
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
    copyRequired(html);
    copyRequired(metadata.dataRoute.slice(1));

    const segments = join(SERVER_APP, routeFile(route, '.segments'));
    if (existsSync(segments)) {
      assertMdreaderCloudflareSourceTree(
        segments,
        `.next/server/app/${routeFile(route, '.segments')}`,
      );
      cpSync(
        segments,
        join(ASSETS, routeFile(route, '.segments')),
        { recursive: true },
      );
    }
  } else {
    copyRequired(routeFile(route, '.body'), route.slice(1));
  }
}

copyRequired('_not-found.html', '404.html');

const assetSet = assertMdreaderCloudflareAssets(ASSETS);

const workerBuild = await build({
  entryPoints: [join(APP, 'cloudflare-worker.ts')],
  outfile: join(OPEN_NEXT, 'worker.js'),
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
  join(OPEN_NEXT, 'worker.meta.json'),
  `${JSON.stringify(workerBuild.metafile, null, 2)}\n`,
);

console.log(
  `Staged ${Object.keys(prerenderManifest.routes).length} MDReader prerenders, ${assetSet.fileCount} bounded assets (${assetSet.totalBytes} bytes), and the bounded CatDB proxy Worker`,
);
