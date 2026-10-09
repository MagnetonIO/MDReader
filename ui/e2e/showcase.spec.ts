/**
 * The P7 showcase surfaces, driven in a REAL browser engine.
 *
 * What this adds over the vitest suite, precisely: **a browser applies CSS.**
 * jsdom parses stylesheets and never evaluates them, which is exactly how a
 * focus-mode bug once shipped — the Related rail stayed on screen because the
 * `display: none` that was supposed to hide it was never computed — and why the
 * repo carries a class-name CSS-drift guard as a stopgap. Here the components
 * mount for real, React runs for real, the stylesheets cascade for real, and the
 * assertions read back computed style, real geometry and the DOM as a genuine
 * mouse click and a genuine text selection left it.
 *
 * What it does NOT do, and must not be described as: it does not drive the
 * native window shell (no titlebar, no traffic lights, no OS chrome), and it
 * does not touch the real Rust engine, the real extractor, or real storage. The
 * IPC boundary is mocked in-page (`src/e2e/harness.ts`) with the deliberately
 * awkward P7 fixture. This is a real-browser UI proof, not an end-to-end one,
 * and it does not stand in for the mac2 GUI leg (see docs/testing/gui-e2e.md).
 *
 * Retrieval and native storage are covered separately by reader-owned service
 * and packaged-client acceptance, rather than a legacy server adapter.
 *
 * Discipline throughout: every content assertion is preceded by a
 * non-emptiness assertion, so an empty render can never satisfy a check.
 */

import { expect, test, type Locator, type Page } from '@playwright/test';

/** Press the shell's primary modifier the same way the app detects it. */
async function usesMeta(page: Page): Promise<boolean> {
  return page.evaluate(() => {
    const platform =
      (navigator as { userAgentData?: { platform?: string } }).userAgentData?.platform ??
      navigator.platform ??
      '';
    return /mac|iphone|ipad|ipod/i.test(platform);
  });
}

async function chord(page: Page, key: string): Promise<void> {
  const meta = await usesMeta(page);
  await page.keyboard.press(`${meta ? 'Meta' : 'Control'}+${key}`);
}

/** Grant the mock workspace and open a document from the tree. */
async function openWorkspace(page: Page, file: string): Promise<void> {
  await page.getByRole('button', { name: /^Open…$/ }).click();
  await expect(page.getByRole('treeitem', { name: 'index.md' })).toBeVisible();
  await page.getByRole('treeitem', { name: file }).click();
  // Non-emptiness first: the reading column has rendered actual blocks.
  await expect(page.locator('.reader-host .mdreader-block').first()).toBeVisible();
}

/** The commands the shell has issued so far (the mock records every one). */
async function sentCommands(page: Page): Promise<string[]> {
  return page.evaluate(() =>
    (window as unknown as { __E2E__: { sent: () => { cmd: string }[] } }).__E2E__
      .sent()
      .map((entry) => entry.cmd),
  );
}

async function clearSentCommands(page: Page): Promise<void> {
  await page.evaluate(() =>
    (window as unknown as { __E2E__: { clearSent: () => void } }).__E2E__.clearSent(),
  );
}

/** One block the reader marked, keyed by the source lines stamped on it. */
interface MarkedBlock {
  start: number;
  end: number;
  text: string;
  /** Every COMPUTED background the block wore while it carried the mark. */
  backgrounds: string[];
}

/**
 * Record every block the reader marks as *located*, in stamped source lines —
 * which is the thing a truncated span loses.
 *
 * Armed BEFORE the click and sampled once per animation frame for a bounded
 * window, because the mark is a ~1.4s pulse: reading the live DOM after the
 * assertions have run finds it already cleared, and a single read at mutation
 * time catches the frame before the cascade has settled on a colour.
 */
async function armLocatedRecorder(page: Page): Promise<void> {
  await page.evaluate(() => {
    interface Entry {
      start: number;
      end: number;
      text: string;
      backgrounds: string[];
    }
    interface Recorder {
      __LOCATED__?: Entry[];
      __LOCATED_STOP__?: number;
      __LOCATED_FRAMES__?: number;
    }
    const store = window as unknown as Recorder;
    store.__LOCATED__ = [];
    store.__LOCATED_FRAMES__ = 0;
    const deadline = performance.now() + 6_000;
    store.__LOCATED_STOP__ = deadline;
    const snap = () => {
      store.__LOCATED_FRAMES__ = (store.__LOCATED_FRAMES__ ?? 0) + 1;
      const seen = store.__LOCATED__!;
      for (const block of document.querySelectorAll('.mdreader-block--located')) {
        const stamped = block.matches('[data-line-start]')
          ? block
          : block.querySelector('[data-line-start]');
        const start = Number(stamped?.getAttribute('data-line-start') ?? NaN);
        const end = Number(stamped?.getAttribute('data-line-end') ?? start);
        const background = getComputedStyle(block).backgroundColor;
        const existing = seen.find((entry) => entry.start === start && entry.end === end);
        if (existing) {
          if (!existing.backgrounds.includes(background)) existing.backgrounds.push(background);
        } else {
          seen.push({
            start,
            end,
            text: (block.textContent ?? '').trim(),
            backgrounds: [background],
          });
        }
      }
      if (store.__LOCATED_STOP__ === deadline && performance.now() < deadline) {
        requestAnimationFrame(snap);
      }
    };
    snap();
  });
}

async function locatedBlocks(page: Page): Promise<MarkedBlock[]> {
  return page.evaluate(
    () => (window as unknown as { __LOCATED__?: MarkedBlock[] }).__LOCATED__ ?? [],
  );
}

/** A real browser's computed `display` for the first match of `selector`. */
async function computedDisplay(page: Page, selector: string): Promise<string | null> {
  return page.evaluate((sel) => {
    const element = document.querySelector(sel);
    return element === null ? null : getComputedStyle(element).display;
  }, selector);
}

/** Adopt `schema` as the reading lens through the Semantic view sheet, then close it. */
async function readAs(page: Page, schema: string): Promise<void> {
  await chord(page, 'Shift+V');
  const sheet = page.getByRole('dialog', { name: 'Semantic view' });
  await expect(sheet).toBeVisible();
  await expect(sheet.locator('.lens-choice')).not.toHaveCount(0);
  await sheet.getByRole('button', { name: new RegExp(schema) }).click();
  await sheet.getByRole('button', { name: 'Read as this' }).click();
  await page.keyboard.press('Escape');
  await expect(sheet).toHaveCount(0);
}

test.beforeEach(async ({ page }) => {
  await page.goto('/e2e.html');
});

// ---------------------------------------------------------------------------
// 1. Semantic view — the band, and what a lens actually changes
// ---------------------------------------------------------------------------

test('Semantic view: the band sits in the reading column, and switching the lens changes what is listed', async ({
  page,
}) => {
  await openWorkspace(page, 'index.md');

  // The prose, before any interpretation is chosen. The whole thesis is that
  // this does not move, so it is captured rather than eyeballed.
  const readerText = page.locator('.reader-host .mdreader__measure');
  await expect(readerText).not.toBeEmpty();
  const proseBefore = (await readerText.innerText()).trim();
  expect(proseBefore.length).toBeGreaterThan(0);

  // No lens in force ⇒ no band. The plain reading gets no banner.
  await expect(page.locator('.lens-band')).toHaveCount(0);

  await chord(page, 'Shift+V');
  const sheet = page.getByRole('dialog', { name: 'Semantic view' });
  await expect(sheet).toBeVisible();

  // Non-emptiness first: there are schemas to choose between at all.
  const choices = sheet.locator('.lens-choice');
  await expect(choices).not.toHaveCount(0);
  expect(await choices.count()).toBeGreaterThanOrEqual(3); // plain + two schemas

  // --- interpretation A: decision_overlay -------------------------------
  await sheet.getByRole('button', { name: /decision_overlay/ }).click();
  await sheet.getByRole('tab', { name: 'Contents' }).click();
  const projected = sheet.locator('.projected');
  await expect(projected).toHaveCount(2);
  await expect(sheet.locator('.contents')).toContainText('Read markdown, not chrome');
  await expect(sheet.locator('.contents')).toContainText('Open files from the tree');

  await sheet.getByRole('button', { name: 'Read as this' }).click();
  await page.keyboard.press('Escape');
  await expect(sheet).toHaveCount(0);

  const band = page.locator('.reader-region .lens-band');
  await expect(band).toBeVisible();
  await expect(band).toContainText('decision_overlay');
  await expect(band).toContainText('2 in the graph');
  // "N claims", never "N awaiting review": proposal_count spans EVERY review
  // state, so calling them all pending would misreport what is left to rule on.
  await expect(band).toContainText('3 claims');
  await expect(band).not.toContainText(/awaiting review/i);
  await expect(band.locator('.lens-band__version')).toHaveText('v1');
  await expect(band).toContainText('Sources untouched');

  // Real layout, not just a DOM position: the band is inside the reading
  // column's box and sits ABOVE the prose. jsdom has no geometry to check this.
  const bandBox = (await band.boundingBox())!;
  const regionBox = (await page.locator('.reader-region').boundingBox())!;
  const bodyBox = (await page.locator('.reader-body').boundingBox())!;
  expect(bandBox.width).toBeGreaterThan(0);
  expect(bandBox.height).toBeGreaterThan(0);
  expect(bandBox.x).toBeGreaterThanOrEqual(regionBox.x - 1);
  expect(bandBox.x + bandBox.width).toBeLessThanOrEqual(regionBox.x + regionBox.width + 1);
  expect(bandBox.y + bandBox.height).toBeLessThanOrEqual(bodyBox.y + 1);

  // --- interpretation B: architecture_overlay ---------------------------
  await chord(page, 'Shift+V');
  await expect(sheet).toBeVisible();
  await sheet.getByRole('button', { name: /architecture_overlay/ }).click();

  // A schema no index has versioned says so — never `v0`, never a blank cell.
  const catalog = sheet.locator('.catalog');
  await expect(catalog).not.toBeEmpty();
  await expect(catalog).toContainText('not yet versioned');
  await expect(catalog).not.toContainText(/\bv0\b/);
  await expect(catalog).toContainText('no checksum yet');

  // The SAME pane, the SAME corpus, a different reading: nothing is listed.
  await sheet.getByRole('tab', { name: 'Contents' }).click();
  await expect(sheet.getByRole('tab', { name: 'Contents' })).toHaveAttribute(
    'aria-selected',
    'true',
  );
  await expect(sheet.locator('.projected')).toHaveCount(0);
  await expect(sheet.locator('.lens-sheet__empty')).toContainText(
    'Nothing has been committed to this schema yet',
  );

  await sheet.getByRole('button', { name: 'Read as this' }).click();
  await page.keyboard.press('Escape');
  await expect(sheet).toHaveCount(0);

  await expect(band).toBeVisible();
  await expect(band).toContainText('architecture_overlay');
  const version = band.locator('.lens-band__version');
  await expect(version).not.toBeEmpty();
  await expect(version).toHaveText('not yet versioned');
  await expect(band).toContainText('0 claims');
  await expect(band).not.toContainText(/awaiting review/i);

  // And the bytes on the page never moved.
  expect((await readerText.innerText()).trim()).toBe(proseBefore);
});

// ---------------------------------------------------------------------------
// 2. Semantic Inspector — the states a tidy fixture would never produce
// ---------------------------------------------------------------------------

/**
 * Open the Inspector and select a real passage by DRAGGING the mouse across the
 * rendered text, the way a reader does.
 *
 * This is real hit-testing over real layout: the drag goes from one laid-out
 * glyph to another and the browser resolves it into a DOM Range, which the shell
 * then reads back as source lines off the blocks the range touched. jsdom has no
 * layout, so it cannot produce a selection this way at all.
 */
async function inspectPassage(page: Page): Promise<Locator> {
  await chord(page, 'Shift+I');
  const rail = page.locator('.lens-rail');
  await expect(rail).toBeVisible();

  const paragraph = page.locator('.reader-host [data-line-start="3"]');
  await expect(paragraph).toBeVisible();
  const box = (await paragraph.boundingBox())!;
  expect(box.width).toBeGreaterThan(0);
  await page.mouse.move(box.x + 4, box.y + box.height / 2);
  await page.mouse.down();
  await page.mouse.move(box.x + 90, box.y + box.height / 2, { steps: 8 });
  await page.mouse.up();
  // The drag really selected text, and the panel really read its lines back.
  expect((await page.evaluate(() => String(window.getSelection()))).trim().length).toBeGreaterThan(
    0,
  );

  const claims = rail.locator('.claim');
  await expect(claims).not.toHaveCount(0);
  return rail;
}

test('Inspector: a claim that is proposed AND in the graph shows BOTH truths', async ({ page }) => {
  await openWorkspace(page, 'index.md');
  const rail = await inspectPassage(page);

  // The passage is named in SOURCE lines, read back off the rendered blocks.
  await expect(rail.locator('.inspector__scope')).toHaveText(
    'Reading claims derived from line 3.',
  );
  await expect(rail.locator('.claim')).toHaveCount(4);

  const divergent = rail.locator('[data-testid="claim-fact-committed-but-proposed"]');
  await expect(divergent).toBeVisible();
  await expect(divergent).not.toBeEmpty();

  // Both readings, side by side. Neither is inferred from the other, and the
  // claim is NOT relabelled approved because a copy of it reached the graph.
  await expect(divergent.locator('.claim__state')).toHaveText('Proposed');
  await expect(divergent.locator('.claim__graph')).toHaveText('in the graph');
  await expect(divergent.locator('.claim__state')).not.toHaveText(/approved/i);
  await expect(divergent.locator('.claim__note')).toContainText(
    'An earlier approval committed this claim',
  );

  // The plainly-proposed claim next to it reads the other way, so the pairing
  // above is a real distinction and not a label every card carries.
  const proposed = rail.locator('[data-testid="claim-fact-proposed"]');
  await expect(proposed.locator('.claim__state')).toHaveText('Proposed');
  await expect(proposed.locator('.claim__graph')).toHaveText('not in the graph');
});

test('Inspector: a stale rejection offers re-review with BOTH digests', async ({ page }) => {
  await openWorkspace(page, 'index.md');
  const rail = await inspectPassage(page);

  const stale = rail.locator('[data-testid="claim-fact-rejected-stale"]');
  await expect(stale).toBeVisible();
  await expect(stale.locator('.claim__state')).toHaveText('Rejected');

  const prompt = stale.locator('.claim__note--rereview');
  await expect(prompt).toBeVisible();
  await expect(prompt).toContainText('You rejected this. The cited text has changed since');

  // Two digests, or a named absence — never one hash presented as if a
  // comparison had been made. This fixture records both, so both must appear.
  const hashes = stale.locator('.claim__hashes');
  await expect(hashes).not.toBeEmpty();
  const hashText = (await hashes.innerText()).trim();
  expect(hashText).toContain('reviewed blake3:');
  expect(hashText).toContain('now blake3:');
  // Both sides present ⇒ no "cannot be compared" note is drawn.
  await expect(stale.locator('.claim__hashes-why')).toHaveCount(0);

  // A confidence the extractor did not record is absent, not zero.
  await expect(stale.locator('.claim__cite')).toContainText('no confidence recorded');
  await expect(stale.locator('.claim__cite')).not.toContainText('0% confident');
});

// ---------------------------------------------------------------------------
// 3. Federated Document — one answer, all of its evidence
// ---------------------------------------------------------------------------

test('Federated Document: result, sources, provenance, and schema stay connected', async ({
  page,
}) => {
  await openWorkspace(page, 'reference.md');
  await clearSentCommands(page);
  await chord(page, 'Shift+I');

  const inspector = page.getByRole('region', { name: 'Federated document' });
  await expect(inspector).toBeVisible();
  await expect(inspector.getByText('Complete')).toBeVisible();
  await expect(inspector.getByText('Reference', { exact: true })).toBeVisible();
  await expect(inspector.getByText('from frontmatter')).toBeVisible();

  await inspector.getByRole('tab', { name: 'Sources' }).click();
  await expect(inspector.locator('.document-sources > li')).toHaveCount(3);
  await expect(inspector.getByText('filesystem')).toBeVisible();
  await expect(inspector.getByText('git')).toBeVisible();
  await expect(inspector.getByText('frontmatter')).toBeVisible();

  await inspector.getByRole('tab', { name: 'Provenance' }).click();
  await expect(inspector.getByText('filesystem.modifiedAt', { exact: true })).toBeVisible();
  await expect(inspector.getByText('git.modifiedAt', { exact: true })).toBeVisible();
  await expect(inspector.getByText('selected', { exact: true })).toHaveCount(2);
  await expect(inspector.getByText('also observed', { exact: true })).toBeVisible();

  await inspector.getByRole('tab', { name: 'Schema' }).click();
  await expect(inspector.getByText('document · v2')).toBeVisible();
  await expect(inspector.getByText(/not the Markdown file or its format/i)).toBeVisible();

  const sent = await sentCommands(page);
  expect(sent.filter((command) => command === 'query_federated_document').length).toBeGreaterThan(0);
});

// ---------------------------------------------------------------------------
// 4. Reviewer identity — inert means nothing crossed the boundary
// ---------------------------------------------------------------------------

test('Reviewer identity: Approve is genuinely inert until a real name is set', async ({ page }) => {
  await openWorkspace(page, 'index.md');
  const rail = await inspectPassage(page);

  const claim = rail.locator('[data-testid="claim-fact-proposed"]');
  const approve = claim.getByRole('button', { name: 'Approve' });
  const reject = claim.getByRole('button', { name: 'Reject' });
  await expect(approve).toBeDisabled();
  await expect(reject).toBeDisabled();
  await expect(approve).toHaveAttribute('title', 'Add your name above to rule on claims');

  // A REAL mouse click at the control's real position — forced past Playwright's
  // actionability wait, so the browser decides what happens, not the test.
  await clearSentCommands(page);
  await approve.click({ force: true });
  await reject.click({ force: true });
  // Nothing crossed the IPC boundary. This is the assertion that separates
  // "styled as unavailable" from "cannot submit".
  expect(await sentCommands(page)).not.toContain('approve_fact');
  expect(await sentCommands(page)).not.toContain('reject_fact');
  await expect(claim.locator('.claim__state')).toHaveText('Proposed');

  // Whitespace is not an identity. The Save control stays inert, and submitting
  // the form anyway does not adopt a signature.
  const nameInput = page.getByLabel('Reviews are signed. Who is ruling on these claims?');
  await nameInput.fill('   ');
  const save = rail.getByRole('button', { name: 'Save' });
  await expect(save).toBeDisabled();
  await nameInput.press('Enter');
  await expect(rail.locator('.inspector__sign')).toBeVisible();
  await expect(rail.locator('.inspector__signer')).toHaveCount(0);
  await expect(approve).toBeDisabled();

  // A real name enables the verbs…
  await nameInput.fill('A. Reader');
  await expect(save).toBeEnabled();
  await save.click();
  await expect(rail.locator('.inspector__signer')).toHaveText('A. Reader');
  await expect(approve).toBeEnabled();

  // …and now a click actually rules: the command is sent, and the panel re-reads
  // the span from storage rather than patching the card in place.
  await clearSentCommands(page);
  await approve.click();
  await expect(claim.locator('.claim__state')).toHaveText('Approved');
  await expect(claim.locator('.claim__graph')).toHaveText('in the graph');
  await expect(claim.locator('.claim__reviewer')).toHaveText('by A. Reader');
  const sent = await sentCommands(page);
  expect(sent).toContain('approve_fact');
  // The re-read, not a local patch: the span was inspected again after ruling.
  expect(sent.indexOf('inspect_span')).toBeGreaterThan(sent.indexOf('approve_fact'));
});

// ---------------------------------------------------------------------------
// 5. Constraint Explorer click-to-source — the load-bearing one
// ---------------------------------------------------------------------------

/**
 * The fixture's violation cites `docs/guide.md:5–7`, which covers TWO stamped
 * blocks: the `## Section A` heading on line 5 and the paragraph on line 7. A
 * jump that carries the range whole marks both; a jump whose end line was
 * dropped anywhere along the way marks only the first, and looks fine.
 */
const SUBJECT_CITATION = 'docs/guide.md:5–7';

async function jumpFromViolation(page: Page): Promise<void> {
  await chord(page, 'Shift+L');
  const sheet = page.getByRole('dialog', { name: 'Constraint explorer' });
  await expect(sheet).toBeVisible();

  // Non-emptiness first: there is a violation, and it is the located one.
  const violations = sheet.locator('.violation');
  await expect(violations).toHaveCount(1);
  await expect(violations.first()).toContainText('architecture_overlay');
  // The failure IS that the left path arrives nowhere — stated, not smoothed.
  await expect(sheet.locator('.located--missing')).toContainText('nowhere — the arrow is missing');

  const jump = sheet.getByRole('button', { name: SUBJECT_CITATION });
  await expect(jump).toBeVisible();
  await armLocatedRecorder(page);
  await jump.click();
  await expect(sheet).toHaveCount(0);
}

/** Assert the reader marked the WHOLE cited range, both ends of it. */
async function expectWholeRangeMarked(page: Page): Promise<void> {
  // Non-emptiness first: something was marked at all, so nothing below can be
  // satisfied by a reader that marked nothing.
  await expect
    .poll(async () => (await locatedBlocks(page)).length, { timeout: 5_000 })
    .toBeGreaterThan(0);
  const marked = await locatedBlocks(page);

  // The END of the chain, read off the DOM: EVERY block the cited span covers,
  // not just the one the reader landed on. Lines 5–7 of guide.md are two
  // stamped blocks; a jump that lost its end line marks only the first.
  expect(marked.map((block) => `${block.start}-${block.end}`)).toEqual(['5-5', '7-7']);
  expect(marked[0]!.text).toContain('Section A');
  expect(marked[1]!.text).toContain('Alpha content.');
  // And no wider than cited: the range is carried whole, not padded to be safe.
  expect(marked.some((block) => block.start <= 3)).toBe(false);
  expect(marked.some((block) => block.start >= 9)).toBe(false);

  // A real engine PAINTED the mark: while marked, each located block resolved a
  // genuine, non-transparent background out of the token cascade
  // (`var(--color-accent-soft)`). jsdom computes nothing of the kind — it is the
  // class name, and only the class name, that a jsdom test can see.
  const opaque = (colour: string) =>
    colour !== '' && colour !== 'transparent' && !/rgba\(\s*0,\s*0,\s*0,\s*0\s*\)/.test(colour);
  await expect
    .poll(async () => (await locatedBlocks(page)).every((b) => b.backgrounds.some(opaque)), {
      timeout: 5_000,
    })
    .toBe(true);
  const painted = await locatedBlocks(page);
  const frames = await page.evaluate(
    () => (window as unknown as { __LOCATED_FRAMES__?: number }).__LOCATED_FRAMES__ ?? 0,
  );
  expect(frames, 'the recorder sampled more than one frame').toBeGreaterThan(1);
  for (const block of painted) {
    expect(block.backgrounds.length).toBeGreaterThan(0);
    expect(
      block.backgrounds.some(opaque),
      `painted backgrounds: ${block.backgrounds.join(', ')}`,
    ).toBe(true);
  }
}

test('Constraint Explorer: a CROSS-DOCUMENT jump marks the whole cited range', async ({ page }) => {
  // Reading index.md; the violation cites guide.md. This is the case that was
  // silently dead — the jump was consumed before its own document had rendered.
  await openWorkspace(page, 'index.md');
  await expect(page.getByRole('heading', { name: 'Welcome' })).toBeVisible();

  await jumpFromViolation(page);

  // The cited DOCUMENT is the one now on screen…
  await expect(page.getByRole('heading', { name: 'Guide' })).toBeVisible();
  await expect(page.getByRole('tab', { name: /guide\.md/ })).toHaveAttribute(
    'aria-selected',
    'true',
  );
  // …and the cited LINES are the ones marked in it.
  await expectWholeRangeMarked(page);

  // The mark is a pulse, not a permanent state: it clears itself afterwards.
  await expect(page.locator('.mdreader-block--located')).toHaveCount(0, { timeout: 5_000 });
});

test('Constraint Explorer: a SAME-DOCUMENT jump marks the whole cited range too', async ({
  page,
}) => {
  // Already reading the cited document: the jump has no load to wait for, and
  // must still mark both ends rather than only the line it lands on.
  await openWorkspace(page, 'guide.md');
  await expect(page.getByRole('heading', { name: 'Guide' })).toBeVisible();
  await expect(page.locator('.mdreader-block--located')).toHaveCount(0);

  await jumpFromViolation(page);

  await expect(page.getByRole('heading', { name: 'Guide' })).toBeVisible();
  await expect(page.getByRole('tab')).toHaveCount(1); // no second tab was opened
  await expectWholeRangeMarked(page);
});

// ---------------------------------------------------------------------------
// 6. Query Explanation — an absent contribution is not a zero one
// ---------------------------------------------------------------------------

test('Query Explanation: a boost that never fired reads "did not fire", not 0', async ({ page }) => {
  await openWorkspace(page, 'index.md');

  await chord(page, 'Shift+K');
  const sheet = page.getByRole('dialog', { name: 'Query explanation' });
  await expect(sheet).toBeVisible();

  // "section" matches several documents, so the ranking has lower places to
  // read as well as a first — only the top hit's semantic arm recorded a rank.
  await sheet.getByLabel('Query to explain').fill('section');
  await sheet.getByRole('button', { name: 'Explain' }).click();

  // Non-emptiness first: there is a ranking to read at all.
  const rows = sheet.locator('.explain');
  await expect(rows).not.toHaveCount(0);
  await expect(rows).toHaveCount(3);
  const row = rows.first();

  const boosts = row.locator('.boost');
  await expect(boosts).toHaveCount(4);

  // The two the fixture never fired are NAMED as absent, with no bar drawn.
  const absent = row.locator('.boost--absent');
  await expect(absent).toHaveCount(2);
  await expect(row.locator('.boost', { hasText: 'Path match' })).toContainText('did not fire');
  await expect(row.locator('.boost', { hasText: 'Graph proximity' })).toContainText('did not fire');

  // The two that DID fire show their real contribution, so "did not fire" is a
  // distinction the panel draws and not the only thing it can say.
  await expect(row.locator('.boost', { hasText: 'Title match' })).toContainText('0.0060');
  await expect(row.locator('.boost', { hasText: 'Recency' })).toContainText('0.0036');

  // Nowhere in the breakdown does an absence read as a contribution of zero.
  const boostText = (await row.locator('.boosts').innerText()).trim();
  expect(boostText.length).toBeGreaterThan(0);
  expect(boostText).not.toContain('0.0000');

  // A hit only one arm found NAMES the arm that missed, rather than leaving a
  // blank rank column — the same distinction the boosts make, one level up.
  const second = rows.nth(1);
  await expect(second.locator('.explain__arms')).not.toBeEmpty();
  await expect(second.locator('.explain__arms')).toHaveText(
    'Only the lexical arm matched this chunk.',
  );
  await expect(second.locator('.explain__ranks')).toContainText('semantic did not match');
  await expect(second.locator('.explain__ranks')).toContainText('lexical #2');

  // Every hit is attributable end to end: chunk → section → document → file.
  await expect(row.locator('.chain')).not.toBeEmpty();
  await expect(row.locator('.chain')).toContainText('chunk');
  await expect(row.locator('.chain')).toContainText('document');
});

// ---------------------------------------------------------------------------
// 6. Focus mode — the assertion jsdom structurally cannot make
// ---------------------------------------------------------------------------

test('Focus mode: the lens surfaces are ACTUALLY not visible, by computed style', async ({
  page,
}) => {
  await openWorkspace(page, 'index.md');
  await readAs(page, 'decision_overlay');
  await chord(page, 'Shift+I');

  const band = page.locator('.lens-band');
  const rail = page.locator('.lens-rail');
  const statusbar = page.locator('.statusbar');

  // Non-emptiness first, and VISIBLE first: there is something to hide.
  await expect(band).toBeVisible();
  await expect(band).not.toBeEmpty();
  await expect(rail).toBeVisible();
  await expect(statusbar).toBeVisible();
  expect(await computedDisplay(page, '.lens-band')).not.toBe('none');
  expect(await computedDisplay(page, '.lens-rail')).not.toBe('none');

  await chord(page, '.');
  await expect(page.locator('.shell--focus')).toHaveCount(1);

  // Still MOUNTED — so this is not the weaker "the element went away" check —
  // and hidden because a real engine computed the cascade. jsdom parses the
  // stylesheet and never evaluates it; that gap is what let a focus-mode
  // regression ship, and this is the assertion that closes it.
  await expect(band).toHaveCount(1);
  await expect(rail).toHaveCount(1);
  expect(await computedDisplay(page, '.lens-band')).toBe('none');
  expect(await computedDisplay(page, '.lens-rail')).toBe('none');
  expect(await computedDisplay(page, '.statusbar')).toBe('none');
  await expect(band).toBeHidden();
  await expect(rail).toBeHidden();
  await expect(statusbar).toBeHidden();
  // Hidden means no box at all, not merely transparent or off-palette.
  expect(await band.boundingBox()).toBeNull();
  expect(await rail.boundingBox()).toBeNull();

  // The collapsed form of the rail is covered too — a strip left on screen is
  // the same bug wearing a smaller element.
  await chord(page, 'Shift+I');
  await expect(page.locator('.lens-strip')).toHaveCount(1);
  expect(await computedDisplay(page, '.lens-strip')).toBe('none');
  await expect(page.locator('.lens-strip')).toBeHidden();

  // The prose stays, and reclaims the width the hidden rails gave up.
  const reader = page.locator('.reader-host');
  await expect(reader).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Welcome' })).toBeVisible();

  // Leaving focus mode brings the interpretation back — the band was hidden,
  // not cleared.
  await page.keyboard.press('Escape');
  await expect(page.locator('.shell--focus')).toHaveCount(0);
  await expect(band).toBeVisible();
  await expect(band).toContainText('decision_overlay');
  expect(await computedDisplay(page, '.lens-band')).not.toBe('none');
});

// ---------------------------------------------------------------------------
// 7. Focus mode — where the reading column actually SITS
// ---------------------------------------------------------------------------

/**
 * The gutters left and right of the reading column, measured against the whole
 * shell rather than against the reader's own box.
 *
 * Measured against `.shell` on purpose. The bug this closes was upstream of the
 * reader: with the sidebar hidden, `main` auto-placed into the leading `auto`
 * grid track and shrink-to-fit, so the reading column filled its own host
 * exactly — left and right gaps of 0 apiece — while sitting flush against the
 * left edge of a window twice its width. Anything that measures the column
 * inside its host calls that centred.
 */
async function readingGutters(page: Page): Promise<{ left: number; right: number; width: number }> {
  const column = (await page.locator('.reader-host .mdreader__measure').boundingBox())!;
  const shell = (await page.locator('.shell').boundingBox())!;
  expect(column.width).toBeGreaterThan(0);
  expect(shell.width).toBeGreaterThan(0);
  return {
    left: column.x - shell.x,
    right: shell.x + shell.width - (column.x + column.width),
    width: column.width,
  };
}

/** Step the reader's text size, through the real control, `steps` times. */
async function stepTextSize(page: Page, direction: 'Larger' | 'Smaller', steps: number) {
  for (let step = 0; step < steps; step += 1) {
    await page.getByRole('button', { name: `${direction} text` }).click();
  }
}

test('Focus mode: the reading column is CENTRED, at the default AND a raised text scale', async ({
  page,
}) => {
  await openWorkspace(page, 'index.md');

  // Non-emptiness first: there is prose, and a column with a real box.
  await expect(page.locator('.reader-host .mdreader__measure')).not.toBeEmpty();

  // Both window widths matter, and for different reasons. In a wide window the
  // failure was the column pinned left with the leftover track empty beside it.
  // In a narrow one it is the measure itself: `--measure` is `70ch` and `ch`
  // grows with the text-size setting, so at the top of the scale the column
  // reaches the usable width, `margin: 0 auto` has nothing left to clamp, and
  // the prose ends up against its own padding.
  for (const width of [1280, 900]) {
    await page.setViewportSize({ width, height: 800 });

    for (const scale of ['default', 'largest'] as const) {
      if (scale === 'largest') await stepTextSize(page, 'Larger', 4);

      await chord(page, '.');
      await expect(page.locator('.shell--focus')).toHaveCount(1);

      const where = `${width}px / ${scale}`;
      // The reader really did reclaim the window: no empty track beside it.
      const host = (await page.locator('.reader-host').boundingBox())!;
      const shell = (await page.locator('.shell').boundingBox())!;
      expect(host.x, `${where}: reader host starts at the shell's left edge`).toBeLessThanOrEqual(
        shell.x + 1,
      );
      expect(host.width, `${where}: reader host spans the shell`).toBeGreaterThanOrEqual(
        shell.width - 1,
      );

      const gutters = await readingGutters(page);
      // Centred: the two gutters agree. A flush-left column reads as left 0 /
      // right ≈ half the window, which is the reported bug exactly.
      expect(
        Math.abs(gutters.left - gutters.right),
        `${where}: gutters ${Math.round(gutters.left)} | ${Math.round(gutters.right)}`,
      ).toBeLessThanOrEqual(2);
      // …and they are real gutters, not a hairline: a column that has run to the
      // window edge is symmetric-at-zero and must not pass.
      expect(gutters.left, `${where}: left gutter`).toBeGreaterThan(24);
      expect(gutters.width, `${where}: column width`).toBeGreaterThan(320);

      await page.keyboard.press('Escape');
      await expect(page.locator('.shell--focus')).toHaveCount(0);
      if (scale === 'largest') await stepTextSize(page, 'Smaller', 4);
    }
  }
});

test('Focus mode in SPLIT: the editor goes with the chrome, and comes back with it', async ({
  page,
}) => {
  await openWorkspace(page, 'index.md');
  await page.getByRole('button', { name: 'Split view' }).click();

  // Non-emptiness first: both panes are really there, each with half the row.
  const source = page.locator('.source-region');
  await expect(source).toBeVisible();
  await expect(page.locator('.source-pane .cm-content')).not.toBeEmpty();
  const editorBefore = await page.locator('.source-pane .cm-content').innerText();
  expect(editorBefore.length).toBeGreaterThan(0);
  const splitHost = (await page.locator('.reader-host').boundingBox())!;
  const shell = (await page.locator('.shell').boundingBox())!;
  expect(splitHost.width).toBeLessThan(shell.width * 0.6); // genuinely sharing

  await chord(page, '.');
  await expect(page.locator('.shell--focus')).toHaveCount(1);

  // Focus is a READING mode: the editor leaves with the rest of the chrome
  // rather than pinning the prose to half a row.
  expect(await computedDisplay(page, '.source-region')).toBe('none');
  await expect(source).toBeHidden();
  expect(await source.boundingBox()).toBeNull();

  const gutters = await readingGutters(page);
  expect(Math.abs(gutters.left - gutters.right)).toBeLessThanOrEqual(2);
  expect(gutters.left).toBeGreaterThan(24);

  // Hidden, never unmounted: the buffer is exactly where it was left.
  await page.keyboard.press('Escape');
  await expect(page.locator('.shell--focus')).toHaveCount(0);
  await expect(source).toBeVisible();
  expect((await page.locator('.source-pane .cm-content').innerText()).trim()).toBe(
    editorBefore.trim(),
  );
});

// ---------------------------------------------------------------------------
// 8. Selecting text with the mouse — the reported gesture, in a real engine
// ---------------------------------------------------------------------------

/** Press, move in stages, release — a genuine selection drag, not `setSelection`. */
async function dragSelect(
  page: Page,
  from: { x: number; y: number },
  to: { x: number; y: number },
): Promise<string> {
  await page.evaluate(() => window.getSelection()?.removeAllRanges());
  await page.mouse.move(from.x, from.y);
  await page.mouse.down();
  await page.mouse.move((from.x + to.x) / 2, (from.y + to.y) / 2, { steps: 6 });
  await page.mouse.move(to.x, to.y, { steps: 6 });
  await page.mouse.up();
  return page.evaluate(() => String(window.getSelection() ?? ''));
}

test('Selection: a drag that starts in a heading’s left gutter selects the heading', async ({
  page,
}) => {
  await openWorkspace(page, 'index.md');

  const heading = page.getByRole('heading', { name: 'Getting started' });
  await expect(heading).toBeVisible();
  const headingText = (await heading.innerText()).trim();
  expect(headingText.length).toBeGreaterThan(0);

  // The section anchor is faded out, not removed — it keeps a box, and that box
  // sits in the left gutter where a select-from-the-left drag begins. Starting
  // the drag on it is the whole point of this test.
  const anchor = heading.locator('a.mdreader-heading__anchor');
  await expect(anchor).toHaveCount(1);
  const anchorBox = (await anchor.boundingBox())!;
  const headingBox = (await heading.boundingBox())!;
  expect(anchorBox.width).toBeGreaterThan(0);
  expect(anchorBox.x + anchorBox.width).toBeLessThanOrEqual(headingBox.x + 1); // truly the gutter

  const selected = await dragSelect(
    page,
    { x: anchorBox.x + anchorBox.width / 2, y: anchorBox.y + anchorBox.height / 2 },
    { x: headingBox.x + headingBox.width * 0.8, y: headingBox.y + headingBox.height / 2 },
  );

  // A hit-testable invisible <a> here swallowed the gesture: no engine starts a
  // text selection from inside a link, so the drag produced nothing at all.
  expect(selected.trim().length, 'the gutter drag selected nothing').toBeGreaterThan(0);
  expect(headingText).toContain(selected.trim());

  // The anchor is hidden from the POINTER, not from assistive technology: it is
  // still in the DOM, still named, and still reachable by keyboard.
  await expect(anchor).toHaveAttribute('aria-label', 'Link to this section');
  await expect(anchor).toHaveAttribute('href', /^#/);
  expect(
    await anchor.evaluate((el) => (el as HTMLElement).tabIndex),
    'the anchor is still keyboard-reachable',
  ).toBeGreaterThanOrEqual(0);
});

test('Selection: a drag across a paragraph selects exactly what it crossed, visibly', async ({
  page,
}) => {
  await openWorkspace(page, 'index.md');

  // A wikilink-dense line — three links in one sentence — begun on plain text.
  const paragraph = page.locator('.reader-host [data-line-start="5"]').first();
  await expect(paragraph).toBeVisible();
  const paragraphText = (await paragraph.innerText()).trim();
  expect(paragraphText.length).toBeGreaterThan(0);
  await expect(paragraph.locator('a.mdreader-wikilink')).not.toHaveCount(0);

  const box = (await paragraph.boundingBox())!;
  const selected = await dragSelect(
    page,
    { x: box.x + 2, y: box.y + box.height / 2 },
    { x: box.x + box.width - 2, y: box.y + box.height / 2 },
  );
  expect(selected.trim().length, 'the paragraph drag selected nothing').toBeGreaterThan(0);
  expect(selected.trim()).toBe(paragraphText);

  // And a successful selection has to LOOK like one. The tint was 1.26:1 against
  // the page, which is a selection that reads as a failed drag; the contrast is
  // asserted here rather than eyeballed because that is how it got shipped.
  const contrast = await page.evaluate(() => {
    const read = (name: string) =>
      getComputedStyle(document.documentElement).getPropertyValue(name).trim();
    const luminance = (hex: string) => {
      const c = hex.replace('#', '');
      const channels = [0, 2, 4]
        .map((i) => parseInt(c.slice(i, i + 2), 16) / 255)
        .map((v) => (v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4));
      return 0.2126 * channels[0]! + 0.7152 * channels[1]! + 0.0722 * channels[2]!;
    };
    const ratio = (a: string, b: string) => {
      const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
      return (hi! + 0.05) / (lo! + 0.05);
    };
    return {
      selectionVsPage: ratio(read('--color-selection'), read('--color-bg')),
      textOnSelection: ratio(read('--color-selection-fg'), read('--color-selection')),
    };
  });
  expect(contrast.selectionVsPage, 'selection tint vs the page').toBeGreaterThan(2);
  expect(contrast.textOnSelection, 'text on the selection tint').toBeGreaterThan(4.5);
});

// ---------------------------------------------------------------------------
// 8. A model that cannot load — reported as itself, over a workspace that works
// ---------------------------------------------------------------------------
//
// The shipping app, launched with no cached model, showed "No folder" and the
// "Open a workspace folder…" placeholder: a real, nameable failure rendered as a
// benign empty state. `?nomodel` puts the mocked backend in exactly that
// condition (see src/e2e/harness.ts) and these assert what the browser actually
// paints — the workspace open and usable, the failure named with its reason, and
// search still answering but marked as text-only.

test('No embedding model: the workspace still opens, and the failure is named — not an empty state', async ({
  page,
}) => {
  await page.goto('/e2e.html?nomodel');
  await page.getByRole('button', { name: /^Open…$/ }).click();

  // The workspace is OPEN: named, listed, and a document renders. Non-emptiness
  // first, so an empty render cannot satisfy any of it.
  await expect(page.locator('.source-switcher__name')).toHaveText('ws');
  await expect(page.getByText('No folder')).toHaveCount(0);
  await expect(page.getByRole('treeitem', { name: 'index.md' })).toBeVisible();
  await page.getByRole('treeitem', { name: 'index.md' }).click();
  await expect(page.locator('.reader-host .mdreader-block').first()).toBeVisible();
  await expect(page.getByText('Open a workspace folder')).toHaveCount(0);

  // The status bar names the real condition, and none of the labels that would
  // misdescribe it. `Local files` is the one the bug actually showed.
  const status = page.locator('.statusbar');
  await expect(status.getByText('Embedding model unavailable')).toBeVisible();
  await expect(status.getByText('Index failed')).toHaveCount(0);
  await expect(status.getByText('Local files')).toHaveCount(0);
  await expect(status.getByText(/^Indexed/)).toHaveCount(0);

  // The reason travels with it, verbatim, on the segment the label sits in.
  const title = await status
    .locator('span[title]')
    .filter({ hasText: 'Embedding model unavailable' })
    .getAttribute('title');
  expect(title).toContain('the embedding model could not be loaded');
  expect(title).toContain('timed out');

  // And there is a way forward, labelled as what it re-attempts.
  await expect(status.getByRole('button', { name: 'Retry model load' })).toBeVisible();
});

test('No embedding model: search still answers, and says its hits are text-only', async ({
  page,
}) => {
  await page.goto('/e2e.html?nomodel');
  await page.getByRole('button', { name: /^Open…$/ }).click();
  await expect(page.getByRole('treeitem', { name: 'index.md' })).toBeVisible();

  await chord(page, 'k');
  const palette = page.locator('.mdsearch');
  await expect(palette).toBeVisible();
  await palette.getByRole('combobox').fill('calm');

  // Non-emptiness first: the lexical arm needs no embedder, so hits DO land.
  await expect(palette.getByRole('option').first()).toBeVisible();

  // …and the list is labelled rather than passed off as the hybrid ranking.
  const notice = palette.getByRole('status');
  await expect(notice).toBeVisible();
  await expect(notice).toContainText('semantic ranking is unavailable');
  await expect(notice).toContainText('the embedding model could not be loaded');
  // A qualification, not a failure: no error banner over the results.
  await expect(palette.getByRole('alert')).toHaveCount(0);

  // The band is genuinely painted (a browser computes CSS; jsdom does not), and
  // sits above the results rather than among them.
  const noticeBox = await notice.boundingBox();
  const firstHitBox = await palette.getByRole('option').first().boundingBox();
  expect(noticeBox, 'the notice has no layout box').not.toBeNull();
  expect(firstHitBox).not.toBeNull();
  expect(noticeBox!.height).toBeGreaterThan(0);
  expect(noticeBox!.y + noticeBox!.height).toBeLessThanOrEqual(firstHitBox!.y + 1);
});

// ---------------------------------------------------------------------------
// 9. A model that is STILL LOADING — the shell never waits for it
// ---------------------------------------------------------------------------
//
// Section 8 covers the settled verdict. This covers the state every launch
// passes through on its way there, and the one a first run on a slow link sits
// in for the whole 120s deadline: `start_import` has been issued and has NOT
// answered. The shipping app was reported blank for exactly that window, so what
// is asserted here is that nothing the user needs is sequenced behind it — the
// tree, the reading column and the keyboard are all live while the import
// promise is still outstanding. `?modelloading` never resolves `start_import`
// (see src/e2e/harness.ts), so any await on it would hang these assertions out.

test('Model still loading: the shell, the tree and reading are all live while the import is unanswered', async ({
  page,
}) => {
  await page.goto('/e2e.html?modelloading');
  await page.getByRole('button', { name: /^Open…$/ }).click();

  // Non-emptiness first: the tree has real rows, and the workspace is named.
  await expect(page.getByRole('treeitem', { name: 'index.md' })).toBeVisible();
  expect(await page.getByRole('treeitem').count()).toBeGreaterThan(0);
  await expect(page.locator('.source-switcher__name')).toHaveText('ws');
  await expect(page.getByText('No folder')).toHaveCount(0);

  // Reading works: a document opens and renders real blocks.
  await page.getByRole('treeitem', { name: 'index.md' }).click();
  await expect(page.locator('.reader-host .mdreader-block').first()).toBeVisible();
  await expect(page.getByText('Open a workspace folder')).toHaveCount(0);

  // The shell is LIVE, not a frozen frame: a keystroke still opens the palette.
  await chord(page, 'p');
  await expect(page.locator('.mdsearch')).toBeVisible();
  await page.keyboard.press('Escape');

  // The status bar is honestly busy — it has not claimed an index that does not
  // exist, and has not pronounced a failure that has not happened.
  const status = page.locator('.statusbar');
  await expect(status.getByText(/^Indexed/)).toHaveCount(0);
  await expect(status.getByText('Index failed')).toHaveCount(0);
  await expect(status.getByText('Embedding model unavailable')).toHaveCount(0);

  // …and this ran against a genuinely PENDING model, not a resolved one: the
  // import was issued, and the listing that paints the tree crossed the boundary
  // BEFORE it. That ordering is the guarantee — a listing sequenced behind the
  // import is a tree that waits on the embedding model.
  const sent = await sentCommands(page);
  expect(sent).toContain('start_import');
  expect(sent).toContain('list_workspace_files');
  expect(sent.indexOf('list_workspace_files')).toBeLessThan(sent.indexOf('start_import'));
});

// --- an unreadable folder is not an empty folder -------------------------
//
// The twelfth instance in this project of a failure rendered as a benign empty
// state, and the first thing a real user hits. A bundled `.app` that macOS has
// not granted a protected folder gets `EPERM` on every `opendir`; the shell
// showed "No markdown files in this folder yet." over a corpus of 49 files, and
// the status bar showed no count at all. A terminal launch inherits the
// terminal's own grants and lists fine, which is why this survived every
// terminal-only check.
//
// `?unreadable` puts the mocked backend in exactly that condition and
// `?emptyfolder` is its control — a folder read successfully that genuinely
// holds nothing. The two must paint DIFFERENTLY, so both are asserted, apart.

test('An unreadable folder says it could not be read, with the reason and the remedy — not "no files"', async ({
  page,
}) => {
  await page.goto('/e2e.html?unreadable');
  await page.getByRole('button', { name: /^Open…$/ }).click();

  // The workspace IS open — this is not the "no folder" state.
  await expect(page.locator('.source-switcher__name')).toHaveText('ws');

  // Non-emptiness first: the failure panel is really painted.
  const panel = page.locator('.filetree--error');
  await expect(panel).toBeVisible();
  await expect(panel).not.toBeEmpty();

  // It states the fact, the folder, the OS's own reason, and the remedy.
  await expect(panel.getByText('This folder could not be read.')).toBeVisible();
  await expect(panel.locator('.filetree__error-path')).toHaveText('/mock/ws');
  await expect(panel.getByText(/Operation not permitted/)).toBeVisible();
  await expect(panel.getByText(/System Settings/)).toBeVisible();

  // And it is announced, not merely drawn.
  await expect(page.getByRole('alert')).toContainText('could not be read');

  // THE REGRESSION: the empty-state sentence must be nowhere on the page.
  await expect(page.getByText('No markdown files in this folder yet.')).toHaveCount(0);

  // The status bar names it too, instead of falling silent.
  const status = page.locator('.statusbar');
  await expect(status.getByText('Folder unreadable')).toBeVisible();
  const title = await status
    .locator('span[title]')
    .filter({ hasText: 'Folder unreadable' })
    .getAttribute('title');
  expect(title).toContain('Operation not permitted');
  expect(title).toContain('System Settings');
});

test('A listing that has not answered yet says so — it does not pre-claim "no files"', async ({
  page,
}) => {
  await page.goto('/e2e.html?listingpending');
  await page.getByRole('button', { name: /^Open…$/ }).click();

  // The folder IS adopted; the read simply has not come back. Observed live in
  // the .app: this is the window macOS holds open while showing its consent
  // prompt, and the app used to fill it with a false claim about the folder.
  await expect(page.locator('.source-switcher__name')).toHaveText('ws');
  await expect(page.locator('.filetree__empty')).toHaveText('Loading…');
  await expect(page.getByText('No markdown files in this folder yet.')).toHaveCount(0);
});

test('A genuinely empty folder still says it is empty — the empty state survives the fix', async ({
  page,
}) => {
  await page.goto('/e2e.html?emptyfolder');
  await page.getByRole('button', { name: /^Open…$/ }).click();

  // The workspace is open, and the tree is the EMPTY tree, not the error panel.
  await expect(page.locator('.source-switcher__name')).toHaveText('ws');
  await expect(page.getByText('No markdown files in this folder yet.')).toBeVisible();

  // It was read successfully — so none of the failure surfaces appear.
  await expect(page.locator('.filetree--error')).toHaveCount(0);
  await expect(page.getByText('This folder could not be read.')).toHaveCount(0);
  await expect(page.locator('.statusbar').getByText('Folder unreadable')).toHaveCount(0);
});
