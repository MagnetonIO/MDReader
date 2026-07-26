/**
 * Playwright global setup: stand up the real `catdb-server` the REAL-ENGINE
 * specs drive, and take it down again when the run ends.
 *
 * Runs for EVERY run, including a mocked-only one. That is deliberate and
 * cheap: the build is incremental, the import is a couple of seconds with the
 * hash embedder, and a mode that only booted when someone remembered to ask for
 * it is a mode that silently stops being exercised. The mocked specs never touch
 * the server, and a boot failure is recorded rather than thrown, so they stay
 * independent of the Rust toolchain (see `catdb-server.ts`).
 */

import {
  cleanWorkspace,
  newManifestPath,
  rmManifest,
  startServer,
  stopServer,
} from './catdb-server.js';

export default async function globalSetup(): Promise<() => Promise<void>> {
  // Publish a manifest path unique to THIS invocation before anything writes
  // one. Playwright carries `process.env` from here into the worker processes,
  // so the specs resolve the same path — and two overlapping runs can no longer
  // land on each other's file (see `manifestPath`).
  process.env.MDREADER_E2E_MANIFEST = newManifestPath();
  // A stale manifest must never be read as this run's result: absent is the
  // honest state until this boot records one.
  await rmManifest();
  await startServer();
  return async () => {
    await stopServer();
    await cleanWorkspace();
    // The path was minted for this run alone, so leaving it behind is litter,
    // not evidence — a failed boot has already been reported by then.
    await rmManifest();
  };
}
