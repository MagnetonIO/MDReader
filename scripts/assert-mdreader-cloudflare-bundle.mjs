import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import {
  mkdtempSync,
  readFileSync,
  rmSync,
  statSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = dirname(dirname(fileURLToPath(import.meta.url)));
const APP = resolve(ROOT, process.argv[2] ?? 'apps/web');
const WORKER = join(APP, '.open-next', 'worker.js');
const WORKER_META = join(APP, '.open-next', 'worker.meta.json');
const ASSETS = join(APP, '.open-next', 'assets');
const WRANGLER = join(APP, 'node_modules', '.bin', 'wrangler');
const FREE_WORKER_GZIP_LIMIT_KIB = 3 * 1024;
const FORBIDDEN_SERVER_INPUT =
  /(?:^|\/)(?:\.open-next\/server-functions\/default|packages\/(?:markdown|ui)|node_modules\/(?:\.pnpm\/)?(?:@shikijs|mermaid|shiki)(?:@|\/))/;

function requireAsset(relativePath) {
  const path = join(ASSETS, relativePath);
  assert.equal(
    statSync(path).isFile(),
    true,
    `Cloudflare asset staging is missing ${relativePath}`,
  );
  return path;
}

const directory = mkdtempSync(join(tmpdir(), 'mdreader-worker-bundle-'));
try {
  const outdir = join(directory, 'bundle');
  const metafile = join(directory, 'bundle-meta.json');
  const output = execFileSync(
    WRANGLER,
    [
      'deploy',
      WORKER,
      '--dry-run',
      '--env',
      'staging',
      '--outdir',
      outdir,
      '--metafile',
      metafile,
    ],
    {
      cwd: APP,
      encoding: 'utf8',
      stdio: ['ignore', 'pipe', 'pipe'],
    },
  );
  const size = output.match(
    /Total Upload:\s+([\d.]+) KiB\s+\/ gzip:\s+([\d.]+) KiB/,
  );
  assert.ok(size, `Wrangler did not report the Worker bundle size:\n${output}`);

  const metadata = JSON.parse(readFileSync(metafile, 'utf8'));
  const stagedMetadata = JSON.parse(readFileSync(WORKER_META, 'utf8'));
  const workerInputs = [
    ...Object.keys(metadata.inputs),
    ...Object.keys(stagedMetadata.inputs),
  ];
  const forbiddenInputs = workerInputs.filter((input) =>
    FORBIDDEN_SERVER_INPUT.test(input),
  );
  assert.deepEqual(
    forbiddenInputs,
    [],
    `reader-only dependencies entered the server Worker:\n${forbiddenInputs.join('\n')}`,
  );

  const gzipKiB = Number(size[2]);
  assert.ok(
    gzipKiB <= FREE_WORKER_GZIP_LIMIT_KIB,
    `MDReader Worker is ${gzipKiB.toFixed(2)} KiB gzip; Cloudflare Workers Free allows ${FREE_WORKER_GZIP_LIMIT_KIB} KiB`,
  );

  const docsHtml = readFileSync(
    requireAsset('docs/markdown-support.html'),
    'utf8',
  );
  assert.match(docsHtml, /class="reader-surface/);
  assert.match(docsHtml, /data-line-start=/);

  const viewerHtml = readFileSync(requireAsset('app.html'), 'utf8');
  assert.match(viewerHtml, /\/api\/catdb/);

  requireAsset('index.html');
  requireAsset('robots.txt');
  requireAsset('sitemap.xml');

  console.log(
    `MDReader Cloudflare Worker: ${size[1]} KiB upload / ${gzipKiB.toFixed(2)} KiB gzip; static reader and viewer assets verified`,
  );
} finally {
  rmSync(directory, { recursive: true, force: true });
}
