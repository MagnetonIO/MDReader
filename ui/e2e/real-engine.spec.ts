/**
 * The showcase surfaces driven in a real browser engine against a REAL ENGINE.
 *
 * `showcase.spec.ts` proves the UI renders and behaves correctly *given*
 * answers. It cannot prove the answers, because it makes them up: its IPC is a
 * fixture, and a fixture can claim any hit, any score, any provenance chain you
 * like. This file drives the same shell with the same browser engines while
 * every answer comes off the wire from a genuinely running `catdb-server` over a
 * real SQLite workspace, indexed from `fixtures/corpus` with the deterministic
 * hash embedder (see `e2e/support/catdb-server.ts` and `src/e2e/real-engine.ts`).
 *
 * WHAT IS PORTED, AND WHY THAT ONE. The mocked suite's Query Explanation test
 * (`showcase.spec.ts` §5) is the only one whose subject is a *claim about where
 * an answer came from* — the chunk → section → document → file → source chain.
 * That is precisely the claim a fixture cannot be trusted with, and precisely
 * the claim P8's decisive test will rest on. So the chain is ported here and
 * asserted against reality: real uuids, a file that genuinely exists in the
 * indexed corpus. The public route deliberately redacts its server filesystem
 * root, so the temp path from setup remains separate test authority and is
 * never copied into an engine provenance field.
 *
 * THE ASSERTION THAT CANNOT BE RELABELLED. Every browser request carries an
 * opaque, per-invocation identity minted by global setup. The loopback server
 * rejects a request for any other run before routing it. The page can use the
 * independently supplied physical root to open files, but engine provenance
 * remains exactly what came over the wire.
 *
 * WHAT THIS DOES NOT PROVE, and must not be described as: it does not drive the
 * native window shell, and it does not exercise the Rust *Tauri command* layer —
 * `catdb-server`'s `/v1` DTOs and the shell's Tauri DTOs are two projections of
 * the same engine call, and this drives the former. See docs/testing/gui-e2e.md.
 */

import { readdir } from 'node:fs/promises';
import path from 'node:path';

import { expect, test, type Page } from '@playwright/test';

import { readManifest, type RealEngineManifest } from './support/catdb-server.js';

/** What the global setup booted. Only ever set once the boot is known good. */
let engine: Extract<RealEngineManifest, { ok: true }>;
/** Every markdown file that genuinely exists under the indexed corpus. */
let corpusFiles: Set<string>;

/** Recursively collect source-relative paths under `root`. */
async function walk(root: string, prefix = ''): Promise<string[]> {
  const entries = await readdir(path.join(root, prefix), { withFileTypes: true });
  const found: string[] = [];
  for (const entry of entries) {
    const relative = prefix === '' ? entry.name : `${prefix}/${entry.name}`;
    if (entry.isDirectory()) found.push(...(await walk(root, relative)));
    else found.push(relative);
  }
  return found;
}

/**
 * FAIL CLOSED. If the server did not start, or did not answer, every test in
 * this file ERRORS naming that. It never degrades to the mocked harness: a
 * real-engine spec that quietly passed with the server down would report success
 * for a path it never exercised.
 */
test.beforeAll(async () => {
  const manifest = await readManifest();
  if (!manifest.ok) {
    throw new Error(
      'REAL-ENGINE MODE could not be established, so these assertions were NOT run against a ' +
        `real engine and must not be reported as passing.\n\nReason: ${manifest.error}`,
    );
  }
  engine = manifest;
  corpusFiles = new Set(await walk(engine.corpusRoot));
  expect(corpusFiles.size, 'the seeded corpus is not empty').toBeGreaterThan(0);
});

/**
 * No test may PASS while the harness was quietly rejecting answers behind it.
 * The assertions above are positive checks; this closes the other door — a run
 * in which the engine stopped answering partway through is not a green run.
 */
test.afterEach(async ({ page }) => {
  const failures = await page.evaluate(
    () =>
      (window as unknown as { __REAL_ENGINE__?: { failures: string[] } }).__REAL_ENGINE__
        ?.failures ?? [],
  );
  expect(failures, 'the real engine answered every command this test issued').toEqual([]);
});

/** Press the shell's primary modifier the same way the app detects it. */
async function chord(page: Page, key: string): Promise<void> {
  const meta = await page.evaluate(() => {
    const platform =
      (navigator as { userAgentData?: { platform?: string } }).userAgentData?.platform ??
      navigator.platform ??
      '';
    return /mac|iphone|ipad|ipod/i.test(platform);
  });
  await page.keyboard.press(`${meta ? 'Meta' : 'Control'}+${key}`);
}

/** What the in-page harness refused to invent, if anything (see real-engine.ts). */
async function engineFailures(page: Page): Promise<string[]> {
  return page.evaluate(
    () =>
      (window as unknown as { __REAL_ENGINE__?: { failures: string[] } }).__REAL_ENGINE__
        ?.failures ?? [],
  );
}

/**
 * Open the real workspace. The physical root and opaque server identity are
 * setup authority; document ids, stored paths, and provenance come from the
 * identity-bound engine process.
 *
 * The wait races the tree against the harness's failure log, so a server that is
 * down or an index that is empty ERRORS BY NAME here. Left as a bare
 * `toBeVisible`, the same conditions read as "element not found", which is the
 * one thing this mode must never let a failure look like.
 */
async function openRealWorkspace(page: Page): Promise<void> {
  await page.addInitScript(({ corpusRoot, serverIdentity }) => {
    (
      window as unknown as {
        __REAL_ENGINE_BOOTSTRAP__?: { corpusRoot: string; serverIdentity: string };
      }
    ).__REAL_ENGINE_BOOTSTRAP__ = { corpusRoot, serverIdentity };
  }, engine);
  await page.goto('/e2e.html?realengine');
  await page.getByRole('button', { name: /^Open…$/ }).click();
  const deadline = Date.now() + 30_000;
  while (Date.now() < deadline) {
    const failures = await engineFailures(page);
    if (failures.length > 0) {
      throw new Error(
        'REAL-ENGINE MODE refused to answer, and did NOT fall back to the fixture harness. ' +
          `These assertions were not run against a real engine:\n\n${failures.join('\n\n')}`,
      );
    }
    // Non-emptiness first: the tree has rows, and they are the indexed corpus.
    if ((await page.getByRole('treeitem').count()) > 0) return;
    await page.waitForTimeout(100);
  }
  throw new Error(
    'the workspace tree never populated within 30s and the harness recorded no failure — ' +
      'the real-engine mode neither answered nor refused, which is a state it must not reach',
  );
}

/** A uuid, as the engine mints them — not a mock's `chunk:/path/to/file`. */
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

// ---------------------------------------------------------------------------
// The ported assertion: the provenance chain, against reality
// ---------------------------------------------------------------------------

test('Query Explanation over a REAL engine: every hit’s chain is a genuine chunk → section → document → file → source out of storage', async ({
  page,
}) => {
  await openRealWorkspace(page);

  await chord(page, 'Shift+K');
  const sheet = page.getByRole('dialog', { name: 'Query explanation' });
  await expect(sheet).toBeVisible();

  // A real term over the real corpus: `hybrid search` is what
  // docs/arch/catdb-engine.md is about, so both retrieval arms have something
  // to find and the fusion is a real fusion.
  await sheet.getByLabel('Query to explain').fill('hybrid search');
  await sheet.getByRole('button', { name: 'Explain' }).click();

  // Non-emptiness first: a ranking came back at all. Nothing below can be
  // satisfied by an empty render — and over an empty index this is where the
  // spec goes red (see the `empty-workspace` fault).
  const rows = sheet.locator('.explain');
  await expect(rows.first()).toBeVisible({ timeout: 30_000 });
  const rowCount = await rows.count();
  // A floor, not just "more than none": every `.some(...)` below has to range
  // over a real ranking rather than over a single row that could satisfy it by
  // accident. The corpus reliably fills the sheet's limit of 10.
  expect(rowCount, 'the real engine returned an explained ranking').toBeGreaterThanOrEqual(5);

  // --- the chain, read off every row --------------------------------------
  interface Chain {
    chunk: string;
    section: string;
    document: string;
    file: string;
    source: string | null;
    text: string;
  }
  const chains: Chain[] = await sheet.locator('.chain').evaluateAll((nodes) =>
    nodes.map((node) => {
      // Each link carries its FULL id in `title`; the visible text is elided.
      const titled = [...node.querySelectorAll('span[title]')].map((span) => ({
        title: span.getAttribute('title') ?? '',
        text: (span.textContent ?? '').trim(),
      }));
      const find = (prefix: string) => titled.find((span) => span.text.startsWith(prefix))?.title;
      return {
        chunk: find('chunk') ?? '',
        section: find('section') ?? '',
        document: find('document') ?? '',
        // The file link is the one whose title equals its own text.
        file: titled.find((span) => span.title === span.text && span.title.endsWith('.md'))?.title ?? '',
        source: find('source') ?? null,
        text: (node.textContent ?? '').trim(),
      };
    }),
  );
  expect(chains.length, 'every explained row drew a provenance chain').toBe(rowCount);

  for (const chain of chains) {
    // Real ids, minted by the engine. The mocked harness's chain reads
    // `chunk:/mock/ws/index.md`; nothing here can be built out of a path.
    expect(chain.chunk, `chunk id in "${chain.text}"`).toMatch(UUID);
    expect(chain.section, `section id in "${chain.text}"`).toMatch(UUID);
    expect(chain.document, `document id in "${chain.text}"`).toMatch(UUID);
    // …and they are three DIFFERENT ids: a chain whose links all point at the
    // same thing is not a chain.
    expect(new Set([chain.chunk, chain.section, chain.document]).size).toBe(3);

    // The file the chain names is a file that GENUINELY EXISTS in the corpus
    // this run indexed — checked against the directory on disk, not against a
    // list the page supplied.
    expect(corpusFiles.has(chain.file), `${chain.file} exists in the indexed corpus`).toBe(true);

    // The public read router intentionally redacts server filesystem roots.
    // Setup's root must not be relabelled as engine provenance, while the
    // engine's source id remains visible in the chain.
    expect(chain.source, `redacted source root in "${chain.text}"`).toBeNull();
    expect(chain.text, `source identity in "${chain.text}"`).toMatch(/source [0-9a-f]{8}/);
    expect(chain.text).not.toContain('no attributing source');
  }

  // The chain's DOCUMENT link is stable per file and distinct across files —
  // the engine's document identity, not a per-row invention.
  const byFile = new Map<string, Set<string>>();
  for (const chain of chains) {
    byFile.set(chain.file, (byFile.get(chain.file) ?? new Set()).add(chain.document));
  }
  for (const [file, ids] of byFile) {
    expect(ids.size, `${file} has one document id across its hits`).toBe(1);
  }

  // --- the index the semantic arm actually searched ------------------------
  const indexLine = sheet.locator('.explain__index').first();
  await expect(indexLine).not.toBeEmpty();
  const indexText = (await indexLine.innerText()).trim();
  // A real index version (a uuid) and the real embedder the workspace was built
  // with. `hash-embedder/test-only` is the engine's own name for it — it appears
  // nowhere in this repository's fixtures.
  expect(indexText).toContain('hash-embedder/test-only');
  expect(indexText, 'the semantic arm ran and named its index').not.toContain(
    'the semantic arm did not run',
  );
  expect(indexText.replace('Semantic index: ', '').split(' · ')[0]).toMatch(UUID);
  // …and it is the embedder the workspace was BUILT with, read independently by
  // the global setup straight from the server (not through the page).
  expect(engine.embeddingModel).toBe('hash-embedder/test-only');

  // --- a real fusion, not a single-arm list -------------------------------
  const arms = await sheet.locator('.explain__arms').allInnerTexts();
  expect(arms.length).toBe(rowCount);
  expect(
    arms.some((sentence) => sentence.includes('Both arms matched')),
    `no hit was found by both arms: ${arms.join(' | ')}`,
  ).toBe(true);

  // --- what the REAL contract says about boosts ---------------------------
  //
  // The mocked test at showcase.spec.ts §5 asserts an absent boost reads "did
  // not fire", not `0.0000`. That distinction is REAL IN THE UI and worth
  // guarding — but it is not reachable from a real backend: `HitBoosts` is four
  // plain `f32`s in the engine (catdb-engine/src/search.rs), so both the Tauri
  // `BoostsDto` and the server's `HitBoostsDto` always carry a number. The
  // `null` the fixture supplies is the fixture's invention. Asserted here so the
  // two modes cannot silently disagree about what the engine can produce.
  const boosts = await sheet.locator('.explain').first().locator('.boost').allInnerTexts();
  expect(boosts.length).toBe(4);
  expect(
    boosts.some((boost) => boost.includes('did not fire')),
    `a real engine reports every boost as a number: ${boosts.join(' | ')}`,
  ).toBe(false);
  // …and at least one of them is a genuine, non-zero contribution, so "all four
  // are numbers" is not satisfied by four zeros.
  expect(
    boosts.some((boost) => /0\.\d*[1-9]/.test(boost)),
    `no boost recorded a non-zero contribution: ${boosts.join(' | ')}`,
  ).toBe(true);
});

// ---------------------------------------------------------------------------
// The search palette, answered by the same real index
// ---------------------------------------------------------------------------

test('Search over a REAL engine: the palette lists hits from documents that genuinely exist in the indexed corpus', async ({
  page,
}) => {
  await openRealWorkspace(page);

  await chord(page, 'k');
  const palette = page.locator('.mdsearch');
  await expect(palette).toBeVisible();
  await palette.getByRole('combobox').fill('hybrid search');

  // Non-emptiness first: the real index answered with real hits.
  await expect(palette.getByRole('option').first()).toBeVisible({ timeout: 30_000 });
  const options = palette.getByRole('option');
  const count = await options.count();
  expect(count, 'the real engine returned hits').toBeGreaterThan(0);

  // Each row is keyed by the hit's CHUNK — a real uuid out of storage, not a
  // path-derived token.
  const values = await options.evaluateAll((nodes) =>
    nodes.map((node) => node.getAttribute('data-value') ?? ''),
  );
  expect(values.length).toBe(count);
  for (const value of values) expect(value).toMatch(UUID);
  expect(new Set(values).size, 'every hit is a distinct chunk').toBe(values.length);

  // And each row names a document that is really in the corpus on disk.
  const titles = await palette.locator('.mdsearch-row__title').allInnerTexts();
  expect(titles.length).toBe(count);
  const basenames = new Set([...corpusFiles].map((file) => file.slice(file.lastIndexOf('/') + 1)));
  for (const title of titles) {
    const name = title.split('·')[0]!.trim();
    expect(basenames.has(name), `${name} is a file in the indexed corpus`).toBe(true);
  }

  // The scores are the engine's, and they are ordered — a real ranking, not a
  // list that happens to have rows in it.
  const scores = (await palette.locator('.mdsearch-row__badge').allInnerTexts()).map(Number);
  expect(scores.length).toBe(count);
  for (const score of scores) expect(Number.isFinite(score)).toBe(true);
  expect(scores).toEqual([...scores].sort((a, b) => b - a));
});
