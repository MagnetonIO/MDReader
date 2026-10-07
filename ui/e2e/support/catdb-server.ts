/**
 * Boot and tear down a REAL `catdb-server` for the Playwright real-engine mode.
 *
 * A spec must never depend on a server someone started by hand: this builds the
 * `catdb` binary, seeds a tempdir workspace from `fixtures/corpus`, indexes it
 * with the deterministic hash embedder (offline, reproducible), starts
 * CatDB's loopback-only, read-only HTTP E2E composition against it,
 * waits until it GENUINELY answers a query, and kills it afterwards. Everything
 * it learned is written to {@link manifestPath} so the spec can read it —
 * including, when the boot failed, the reason it failed.
 *
 * This deliberately does not invoke production `catdb serve`: V1's
 * authenticated network gateway refuses legacy search/read DTOs because they
 * cannot express source and field authorization. The E2E example mounts only
 * the feature-gated E2E hybrid read router, with no mutations, index
 * operations, MCP, or filesystem roots, and refuses any non-loopback bind.
 * Its deterministic hash embedder is reproducible test evidence only, not
 * semantic-provider parity.
 *
 * FAIL CLOSED, IN TWO LAYERS.
 *  1. Here: any failure is recorded as `{ ok: false, error }`, and
 *     `real-engine.spec.ts` throws that reason out of its `beforeAll`. The
 *     failure is scoped to the real-engine specs rather than aborting the whole
 *     run, because the MOCKED specs are deliberately independent of Rust, of
 *     indexing, and of this file — they must keep running on a machine with no
 *     toolchain.
 *  2. In the page: `src/e2e/real-engine.ts` rejects every command it cannot
 *     answer from the live server. So even a spec that skipped layer 1 gets a
 *     hard error, never fixture data.
 *
 * FAULT INJECTION (the same discipline as `MDREADER_SMOKE_FAULT` in the native
 * leg): `MDREADER_E2E_CATDB_FAULT` turns the real-engine assertions RED on
 * purpose, to prove they are load-bearing.
 *  - `empty-workspace` — point the SERVER at a workspace indexed from an empty
 *    directory, while the corpus is still seeded on disk exactly as usual. The
 *    server starts and answers; it simply knows nothing. Everything the spec
 *    reads out of the UI must go red, and it must go red *there* — the corpus
 *    the spec checks files against is untouched, so a failure cannot be the
 *    setup noticing on the UI's behalf.
 *  - `server-down` — boot, verify it answers, then KILL it before the specs run.
 *    The specs must error naming that, never silently fall back to mocks.
 */

import { spawn, type ChildProcess } from 'node:child_process';
import { execFile } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { createServer } from 'node:net';
import { cp, mkdir, mkdtemp, readFile, realpath, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';

const run = promisify(execFile);

const here = path.dirname(fileURLToPath(import.meta.url));

/** The monorepo root (`apps/mdreader/ui/e2e/support` → five levels up). */
export const repoRoot = path.resolve(here, '../../..');

/**
 * The port the E2E server binds, and the port `vite.config.ts` proxies
 * `/__catdb` to. Both read the same variable with the same default, so the two
 * processes agree without Playwright having to hand a value to the dev server
 * (which `reuseExistingServer` would defeat anyway).
 */
export const catdbPort = Number(process.env.MDREADER_E2E_CATDB_PORT ?? 8788);

/**
 * Where this run's boot result is left for the specs.
 *
 * Keyed to the INVOCATION, not to the port. Keyed to the port, two overlapping
 * runs share one file: the second run's server fails to bind, and its
 * `{ ok: false }` lands on top of the first run's healthy manifest — so the
 * FIRST run's specs fail, for a reason that has nothing to do with them. That is
 * still fail-closed, but it fails the wrong run and reads as a mystery.
 *
 * `global-setup.ts` mints the path and publishes it here; Playwright carries
 * `process.env` from the global setup into the worker processes, which is how
 * the specs find it. The port-keyed name remains only as the fallback for a spec
 * run without that setup — where the manifest will be absent anyway, and absent
 * is correctly read as "the boot never happened".
 */
export function manifestPath(): string {
  return (
    process.env.MDREADER_E2E_MANIFEST ??
    path.join(tmpdir(), `mdreader-e2e-real-engine-${catdbPort}.json`)
  );
}

/** A manifest path unique to this invocation (see {@link manifestPath}). */
export function newManifestPath(): string {
  const token = `${process.pid}-${Math.random().toString(36).slice(2, 10)}`;
  return path.join(tmpdir(), `mdreader-e2e-real-engine-${token}.json`);
}

/** What the global setup learned, good or bad. */
export type RealEngineManifest =
  | {
      ok: true;
      /** The tempdir corpus that was indexed — a path invented at run time. */
      corpusRoot: string;
      /** The SQLite workspace the server was pointed at. */
      dbPath: string;
      /** `http://127.0.0.1:<port>` — the server's own origin. */
      origin: string;
      /** Distinct documents a `type:document` probe found in the index. */
      documentsIndexed: number;
      /** Files the import reported scanning. */
      filesImported: number;
      /** The embedding model the index records (`hash-embedder/test-only`). */
      embeddingModel: string | null;
      /** The fault injected, if any. */
      fault: string | null;
      /** Opaque invocation token required by every request to this server. */
      serverIdentity: string;
      pid: number | null;
    }
  | { ok: false; error: string; fault: string | null };

async function writeManifest(manifest: RealEngineManifest): Promise<void> {
  await writeFile(manifestPath(), JSON.stringify(manifest, null, 2), 'utf8');
}

/** Drop any manifest an earlier run left, so absent means "not booted yet". */
export async function rmManifest(): Promise<void> {
  await rm(manifestPath(), { force: true });
}

/** Read what the global setup left. Absent ⇒ it never ran, which is a failure. */
export async function readManifest(): Promise<RealEngineManifest> {
  try {
    return JSON.parse(await readFile(manifestPath(), 'utf8')) as RealEngineManifest;
  } catch (cause) {
    return {
      ok: false,
      fault: null,
      error:
        `no real-engine manifest at ${manifestPath()} — the Playwright global setup ` +
        `(e2e/support/global-setup.ts) did not run or crashed before it could record anything ` +
        `(${cause instanceof Error ? cause.message : String(cause)})`,
    };
  }
}

/** One `/v1` call straight from Node (no proxy, no browser). */
async function api<T>(
  origin: string,
  serverIdentity: string,
  route: string,
  body?: unknown,
): Promise<T> {
  const response = await fetch(
    `${origin}${route}`,
    body === undefined
      ? { method: 'GET', headers: { 'x-mdreader-e2e-run': serverIdentity } }
      : {
          method: 'POST',
          headers: {
            'content-type': 'application/json',
            'x-mdreader-e2e-run': serverIdentity,
          },
          body: JSON.stringify(body),
        },
  );
  const text = await response.text();
  if (!response.ok) throw new Error(`${route} → ${response.status}: ${text.slice(0, 300)}`);
  return JSON.parse(text) as T;
}

interface ProbeHit {
  file_path: string;
  explain?: { embedding_model?: string; provenance: { source_root?: string } };
}

interface ProbeDocuments {
  count: number;
  documents: { file_path: string }[];
}

/**
 * Whether anything already holds {@link catdbPort}.
 *
 * Checked BEFORE anything is built or written, because the port is fixed by
 * default: `vite.config.ts` and this file are separate processes, so they agree
 * on a proxy target only by both reading the same variable with the same
 * default. That makes an overlapping run a real possibility, and the operator
 * deserves to be told which knob fixes it rather than reading a bind error out
 * of a spawn log.
 */
async function portIsFree(): Promise<boolean> {
  return new Promise((resolve) => {
    const probe = createServer();
    probe.once('error', () => resolve(false));
    probe.once('listening', () => probe.close(() => resolve(true)));
    probe.listen(catdbPort, '127.0.0.1');
  });
}

/** The live handle the teardown needs. */
let child: ChildProcess | null = null;
let workspaceDir: string | null = null;

/** SIGTERM the server, escalate if needed, and do not return before it exits. */
export async function stopServer(): Promise<void> {
  const process_ = child;
  child = null;
  if (process_ === null || process_.exitCode !== null || process_.signalCode !== null) return;
  const ended = new Promise<void>((resolve) => process_.once('exit', () => resolve()));
  process_.kill('SIGTERM');
  const exitedAfterTerm = await Promise.race([
    ended.then(() => true),
    new Promise<false>((resolve) => setTimeout(() => resolve(false), 5_000)),
  ]);
  if (exitedAfterTerm) return;
  if (!process_.kill('SIGKILL')) {
    throw new Error(`could not SIGKILL MDReader E2E server process ${process_.pid ?? 'unknown'}`);
  }
  const exitedAfterKill = await Promise.race([
    ended.then(() => true),
    new Promise<false>((resolve) => setTimeout(() => resolve(false), 5_000)),
  ]);
  if (!exitedAfterKill) {
    throw new Error(
      `MDReader E2E server process ${process_.pid ?? 'unknown'} did not exit after SIGKILL`,
    );
  }
}

/** Remove the tempdir workspace. */
export async function cleanWorkspace(): Promise<void> {
  const dir = workspaceDir;
  workspaceDir = null;
  if (dir !== null) await rm(dir, { recursive: true, force: true });
}

/**
 * Build, seed, index, serve, and prove it answers. Records the outcome either
 * way; never throws (see the two-layer note at the top of this file).
 */
export async function startServer(): Promise<void> {
  const fault = process.env.MDREADER_E2E_CATDB_FAULT ?? null;
  const origin = `http://127.0.0.1:${catdbPort}`;
  const serverIdentity = randomUUID();
  const log: string[] = [];
  try {
    // 0. Is the port ours to take? A second run on one machine would otherwise
    //    fail to bind halfway through and report it as a spawn log.
    if (!(await portIsFree())) {
      throw new Error(
        `127.0.0.1:${catdbPort} is already in use, so this run cannot start its own ` +
          'catdb-server. Another Playwright run is probably still going, or a stray `catdb ' +
          'serve` is holding the port. This suite uses fixed, non-reusable Vite and CatDB ' +
          'listeners so an overlapping run fails closed instead of querying the wrong process.',
      );
    }

    // 1. The binary. Built here rather than assumed, so a stale or missing
    //    target directory is a named failure and not a mystery.
    await run(
      'cargo',
      [
        'build',
        '-p',
        'mdreader-ingest',
        '--bin',
        'catdb',
        '--example',
        'e2e_read_only_server',
        '--features',
        'e2e-test-support',
      ],
      {
        cwd: repoRoot,
        maxBuffer: 32 * 1024 * 1024,
      },
    );
    const bin = path.join(repoRoot, 'target', 'debug', 'catdb');
    const serverBin = path.join(repoRoot, 'target', 'debug', 'examples', 'e2e_read_only_server');

    // 2. A tempdir workspace, seeded from the real corpus. The root is invented
    //    at run time on purpose: no fixture in the repo can name it, so a
    //    workspace path appearing in the UI is itself evidence of a real index.
    //    `realpath` because the ingest records the CANONICAL root, and on macOS
    //    `os.tmpdir()` is `/var/…`, a symlink to `/private/var/…`. The manifest
    //    must carry the string the engine will actually report, or the spec
    //    would compare two spellings of the same directory.
    workspaceDir = await realpath(await mkdtemp(path.join(tmpdir(), 'mdreader-e2e-catdb-')));
    const corpusRoot = path.join(workspaceDir, 'corpus');
    await cp(path.join(repoRoot, 'fixtures', 'corpus'), corpusRoot, { recursive: true });

    // The RED proof: the corpus above is seeded exactly as usual — so the spec's
    // own on-disk checks still have real files to work with — but the ENGINE is
    // pointed at an empty directory. Anything the spec reads out of the UI must
    // go red, and it cannot be the setup failing on the UI's behalf.
    const indexRoot = path.join(workspaceDir, 'empty');
    if (fault === 'empty-workspace') await mkdir(indexRoot, { recursive: true });

    // 3. Index it offline and deterministically — `--embedder hash` never
    //    downloads or loads a model, so this is reproducible and CI-safe.
    const dbPath = path.join(workspaceDir, 'catdb.db');
    const imported = await run(
      bin,
      [
        'import',
        fault === 'empty-workspace' ? indexRoot : corpusRoot,
        '--db',
        dbPath,
        '--embedder',
        'hash',
        '--json',
      ],
      { cwd: repoRoot, maxBuffer: 32 * 1024 * 1024 },
    );
    const report = JSON.parse(imported.stdout) as { files_scanned: number };

    // 4. Serve through the bounded read-only HTTP router. The example opens the
    // same embedded engine, refuses non-loopback addresses, and requires this
    // run's opaque identity on every request.
    child = spawn(
      serverBin,
      [`127.0.0.1:${catdbPort}`, dbPath, serverIdentity],
      {
        cwd: repoRoot,
        stdio: ['ignore', 'pipe', 'pipe'],
      },
    );
    child.stdout?.on('data', (chunk: Buffer) => log.push(chunk.toString()));
    child.stderr?.on('data', (chunk: Buffer) => log.push(chunk.toString()));
    let exited: string | null = null;
    child.once('exit', (code, signal) => {
      exited = `the E2E read-only server exited early (code ${code}, signal ${signal})`;
    });
    // A spawn that never starts (missing binary, not executable) emits `error`,
    // not `exit`; unhandled, that would take the whole Playwright runner down
    // instead of being recorded as a boot failure.
    child.once('error', (cause) => {
      exited = `the E2E read-only server could not be spawned: ${cause.message}`;
    });

    // 5. Wait until it GENUINELY answers — not until the port is open. The probe
    //    is a real query whose result the specs depend on.
    const deadline = Date.now() + 60_000;
    let probe: ProbeDocuments | null = null;
    let lastError = 'never attempted';
    while (Date.now() < deadline) {
      if (exited !== null) throw new Error(`${exited}\n${log.join('').slice(-2000)}`);
      try {
        probe = await api<ProbeDocuments>(origin, serverIdentity, '/v1/documents');
        break;
      } catch (cause) {
        lastError = cause instanceof Error ? cause.message : String(cause);
        await new Promise((resolve) => setTimeout(resolve, 250));
      }
    }
    if (probe === null) {
      throw new Error(`the E2E read-only server never answered ${origin}/v1/documents: ${lastError}`);
    }

    // The identity must be an enforced boundary, not merely a value both sides
    // happen to carry. A request from another invocation must fail before any
    // read route answers.
    const foreign = await fetch(`${origin}/v1/documents`, {
      headers: { 'x-mdreader-e2e-run': randomUUID() },
    });
    if (foreign.status !== 412) {
      throw new Error(
        `the E2E server answered a foreign run identity with ${foreign.status}; expected 412`,
      );
    }

    const documents = new Set(probe.documents.map((document) => document.file_path));

    // Which embedder the index was actually built with. Document enumeration
    // does not run retrieval, hence a second probe with real TERMS, which is
    // also the strongest liveness check available: it exercises both retrieval
    // arms and the fusion.
    // Best-effort, because an empty index legitimately has nothing to say.
    let embeddingModel: string | null = null;
    try {
      const terms = await api<{ hits: ProbeHit[] }>(origin, serverIdentity, '/v1/search', {
        q: 'hybrid search',
        limit: 10,
        explain: true,
      });
      embeddingModel =
        terms.hits.find((hit) => hit.explain?.embedding_model)?.explain?.embedding_model ?? null;
    } catch {
      // Left null; the spec asserts what it needs and fails naming the absence.
    }

    // `empty-workspace` is allowed through DELIBERATELY: the server is up and
    // answering, so the point of the fault is that everything downstream — the
    // in-page harness and the spec's own assertions — must go red over an empty
    // index. Hard-failing here would hide which layer actually catches it.
    if (documents.size === 0 && fault !== 'empty-workspace') {
      throw new Error(
        `catdb-server answered, but its index holds no documents (import scanned ` +
          `${report.files_scanned} files from ${corpusRoot})`,
      );
    }

    if (fault === 'server-down') {
      // Proven alive, then taken away: the specs must ERROR, not pass.
      await stopServer();
    }

    await writeManifest({
      ok: true,
      corpusRoot,
      dbPath,
      origin,
      documentsIndexed: documents.size,
      filesImported: report.files_scanned,
      embeddingModel,
      fault,
      serverIdentity,
      pid: child?.pid ?? null,
    });
  } catch (cause) {
    await stopServer();
    await writeManifest({
      ok: false,
      fault,
      error:
        `${cause instanceof Error ? cause.message : String(cause)}` +
        (log.length > 0 ? `\n--- E2E read-only server output ---\n${log.join('').slice(-2000)}` : ''),
    });
  }
}
