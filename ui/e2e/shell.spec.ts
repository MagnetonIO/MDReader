import { expect, test } from '@playwright/test';

test.beforeEach(async ({ page }) => {
  await page.goto('/e2e.html');
});

test('grant → index progress → open → wikilink navigation (resolved + unresolved)', async ({
  page,
}) => {
  await page.getByRole('button', { name: /^Open…$/ }).click();
  await expect(page.getByRole('treeitem', { name: 'index.md' })).toBeVisible();

  // The StatusBar transitions from the busy import phase to the finished index.
  await expect(page.getByText(/Indexing/)).toBeVisible();
  await expect(page.getByText(/Indexed 3 files/)).toBeVisible();

  // Open the note that carries the wikilinks.
  await page.getByRole('treeitem', { name: 'index.md' }).click();
  await expect(page.getByRole('heading', { name: 'Welcome' })).toBeVisible();

  // A broken wikilink stays put and raises a toast (muted, non-navigating).
  await page.getByRole('link', { name: 'Nonexistent' }).click();
  await expect(page.getByText(/No note named "Nonexistent"/)).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Welcome' })).toBeVisible();

  // A resolved wikilink navigates the tab to the target note (through the index).
  await page.getByRole('link', { name: 'Guide', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Guide' })).toBeVisible();
});

test('a wikilink with a heading navigates and scrolls to that section', async ({ page }) => {
  await page.getByRole('button', { name: /^Open…$/ }).click();
  await page.getByRole('treeitem', { name: 'index.md' }).click();
  await expect(page.getByRole('heading', { name: 'Welcome' })).toBeVisible();

  // [[Guide#Section B]] resolves to guide.md AND scrolls to the Section B heading
  // (the index returns the heading's source line; the reader jumps to it).
  await page.getByRole('link', { name: 'Guide#Section B' }).click();
  await expect(page.getByRole('heading', { name: 'Guide' })).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Section B' })).toBeInViewport();
});

test('open from tree → render → outline-jump → tab lifecycle', async ({ page }) => {
  // Welcome state before any folder is open.
  await expect(page.getByText(/A calm place to read your markdown/i)).toBeVisible();

  // Open the (mocked) workspace folder; the tree populates.
  await page.getByRole('button', { name: /^Open…$/ }).click();
  await expect(page.getByRole('treeitem', { name: 'index.md' })).toBeVisible();

  // Open a document from the tree — the real pipeline renders its heading.
  await page.getByRole('treeitem', { name: 'index.md' }).click();
  await expect(page.getByRole('heading', { name: 'Welcome' })).toBeVisible();

  // The outline lists the document's headings.
  await expect(page.getByRole('link', { name: 'Getting started' })).toBeVisible();

  // Open a second document → two tabs.
  await page.getByRole('treeitem', { name: 'guide.md' }).click();
  await expect(page.getByRole('heading', { name: 'Guide' })).toBeVisible();
  await expect(page.getByRole('tab')).toHaveCount(2);

  // Outline-jump within the active document.
  await page.getByRole('link', { name: 'Section B' }).click();
  await expect(page.getByRole('heading', { name: 'Section B' })).toBeInViewport();

  // Close the active tab → back to one.
  await page.getByRole('button', { name: 'Close guide.md' }).click();
  await expect(page.getByRole('tab')).toHaveCount(1);
  await expect(page.getByRole('heading', { name: 'Welcome' })).toBeVisible();
});

/** Press the shell's primary modifier the same way the app detects it. */
async function usesMeta(page: import('@playwright/test').Page): Promise<boolean> {
  return page.evaluate(() => {
    const platform =
      (navigator as { userAgentData?: { platform?: string } }).userAgentData?.platform ??
      navigator.platform ??
      '';
    return /mac|iphone|ipad|ipod/i.test(platform);
  });
}

async function openThreeDocs(page: import('@playwright/test').Page): Promise<void> {
  await page.getByRole('button', { name: /^Open…$/ }).click();
  await page.getByRole('treeitem', { name: 'index.md' }).click();
  await page.getByRole('treeitem', { name: 'guide.md' }).click();
  await page.getByRole('treeitem', { name: 'reference.md' }).click();
  await expect(page.getByRole('tab')).toHaveCount(3);
}

test('double-click pins a tab and regroups it to the left', async ({ page }) => {
  await openThreeDocs(page);
  // The active (rightmost) tab is reference.md; pinning it regroups it far left.
  await page.getByRole('tab', { name: /reference\.md/ }).dblclick();
  await expect(page.getByRole('tab').first()).toContainText('reference.md');
  await expect(page.getByRole('tab').first()).toHaveClass(/tab--pinned/);
});

test('close then reopen restores the tab in place (Mod+Shift+T)', async ({ page }) => {
  await page.getByRole('button', { name: /^Open…$/ }).click();
  await page.getByRole('treeitem', { name: 'index.md' }).click();
  await page.getByRole('treeitem', { name: 'guide.md' }).click();
  await expect(page.getByRole('tab')).toHaveCount(2);

  await page.getByRole('button', { name: 'Close guide.md' }).click();
  await expect(page.getByRole('tab')).toHaveCount(1);

  const meta = await usesMeta(page);
  await page.keyboard.press(meta ? 'Meta+Shift+T' : 'Control+Shift+T');
  await expect(page.getByRole('tab')).toHaveCount(2);
  await expect(page.getByRole('heading', { name: 'Guide' })).toBeVisible();
});

test('Ctrl+Tab cycles tabs by most-recent use', async ({ page }) => {
  await openThreeDocs(page);
  // MRU is [reference, guide, index]; one Ctrl+Tab lands on the previous doc.
  await expect(page.getByRole('heading', { name: 'Reference' })).toBeVisible();
  await page.keyboard.press('Control+Tab');
  await expect(page.getByRole('heading', { name: 'Guide' })).toBeVisible();
});

test('Cmd+K search → result → Enter opens the doc at its provenance section', async ({ page }) => {
  await page.getByRole('button', { name: /^Open…$/ }).click();
  await expect(page.getByRole('treeitem', { name: 'index.md' })).toBeVisible();

  const meta = await usesMeta(page);
  await page.keyboard.press(meta ? 'Meta+k' : 'Control+k');
  const input = page.getByRole('combobox');
  await expect(input).toBeVisible();

  // "beta" sits in guide.md's `## Section B`; the hit's start_line points there.
  await input.fill('beta');
  await expect(page.getByRole('option').filter({ hasText: 'guide.md' })).toBeVisible();

  // Enter opens the doc tab and lands on the provenance section (Section B).
  await page.keyboard.press('Enter');
  await expect(page.getByRole('heading', { name: 'Guide' })).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Section B' })).toBeInViewport();
});

test('search intents: Best is default, Exact is clickable, ⌘/Ctrl+3 selects', async ({ page }) => {
  await page.getByRole('button', { name: /^Open…$/ }).click();
  await expect(page.getByRole('treeitem', { name: 'index.md' })).toBeVisible();

  const meta = await usesMeta(page);
  await page.keyboard.press(meta ? 'Meta+k' : 'Control+k');
  await expect(page.getByRole('tab', { name: 'Best' })).toHaveAttribute('aria-selected', 'true');

  await page.getByRole('tab', { name: 'Exact' }).click();
  await expect(page.getByRole('tab', { name: 'Exact' })).toHaveAttribute('aria-selected', 'true');

  // The intent shortcut (⌘/Ctrl+3) jumps straight to Semantic — bare Tab does not
  // cycle intents (it moves focus, keeping the breadcrumb buttons reachable).
  await page.keyboard.press(meta ? 'Meta+3' : 'Control+3');
  await expect(page.getByRole('tab', { name: 'Semantic' })).toHaveAttribute('aria-selected', 'true');
});

test('edit → save clears the dirty marker; split view renders source + preview', async ({
  page,
}) => {
  await page.getByRole('button', { name: /^Open…$/ }).click();
  await page.getByRole('treeitem', { name: 'reference.md' }).click();
  await expect(page.getByRole('heading', { name: 'Reference' })).toBeVisible();

  // Enter source view; the CodeMirror editor mounts with the document text.
  await page.getByRole('button', { name: 'Source view' }).click();
  const editor = page.locator('.cm-content');
  await expect(editor).toBeVisible();
  await expect(editor).toContainText('Reference material.');

  // Type at the end → the tab shows the unsaved-changes dot.
  await editor.click();
  const meta = await usesMeta(page);
  await page.keyboard.press(meta ? 'Meta+ArrowDown' : 'Control+End');
  await page.keyboard.type('\n\nAn appended edit.\n');
  await expect(page.locator('.tab__dirty')).toBeVisible();

  // Cmd+S saves (hash-guarded); the dirty dot clears, no conflict banner appears.
  await page.keyboard.press(meta ? 'Meta+s' : 'Control+s');
  await expect(page.locator('.tab__dirty')).toHaveCount(0);
  await expect(page.getByRole('alert')).toHaveCount(0);

  // Split view shows the editor AND a live preview of the buffer side by side.
  await page.getByRole('button', { name: 'Split view' }).click();
  await expect(page.locator('.reader-body--split .source-region .cm-content')).toBeVisible();
  await expect(
    page.locator('.reader-body--split .reader-host').getByRole('heading', { name: 'Reference' }),
  ).toBeVisible();
});

test('a save that hits an external change raises the conflict banner; Reload adopts disk', async ({
  page,
}) => {
  await page.getByRole('button', { name: /^Open…$/ }).click();
  await page.getByRole('treeitem', { name: 'reference.md' }).click();
  await expect(page.getByRole('heading', { name: 'Reference' })).toBeVisible();

  await page.getByRole('button', { name: 'Source view' }).click();
  const editor = page.locator('.cm-content');
  await expect(editor).toContainText('Reference material.');

  // Make a local edit…
  await editor.click();
  const meta = await usesMeta(page);
  await page.keyboard.press(meta ? 'Meta+ArrowDown' : 'Control+End');
  await page.keyboard.type('\n\nMy local edit.\n');

  // …then simulate the file changing on disk out from under us (bumps its hash).
  await page.evaluate(() => {
    (
      window as unknown as { __E2E__: { writeDisk: (p: string, c: string) => void } }
    ).__E2E__.writeDisk('/mock/ws/docs/reference.md', '# Reference\n\nChanged elsewhere.\n');
  });

  // Saving now trips the guard → the non-blocking conflict banner (save flavour).
  await page.keyboard.press(meta ? 'Meta+s' : 'Control+s');
  const banner = page.getByRole('alert');
  await expect(banner).toContainText(/changed on disk since you opened it/i);

  // Reload discards the local buffer and adopts the on-disk version; banner clears.
  await banner.getByRole('button', { name: 'Reload' }).click();
  await expect(page.getByRole('alert')).toHaveCount(0);
  await expect(page.locator('.cm-content')).toContainText('Changed elsewhere.');
});

test('macOS: Ctrl+E in the source editor is NOT the mode toggle (stays CM6 line-end)', async ({
  page,
}) => {
  const meta = await usesMeta(page);
  test.skip(!meta, 'the ⌘-only chord reservation only applies on macOS');

  await page.getByRole('button', { name: /^Open…$/ }).click();
  await page.getByRole('treeitem', { name: 'reference.md' }).click();
  await page.getByRole('button', { name: 'Source view' }).click();
  await expect(page.locator('.cm-content')).toBeVisible();
  await page.locator('.cm-content').click();

  // Ctrl+E on macOS must NOT toggle back to reading (that chord is ⌘E); CM6 keeps
  // Ctrl+E as its native cursor-to-line-end. The editor stays in source view.
  await page.keyboard.press('Control+e');
  await expect(page.locator('.cm-content')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Source view' })).toHaveAttribute(
    'aria-pressed',
    'true',
  );
});

test('split view: scrolling the editor drives the preview (approximate one-way sync)', async ({
  page,
}) => {
  await page.getByRole('button', { name: /^Open…$/ }).click();
  await page.getByRole('treeitem', { name: 'long.md' }).click();
  // Wait for the document to open (the Split toggle enables once a doc is active).
  await expect(page.getByRole('button', { name: 'Split view' })).toBeEnabled();

  await page.getByRole('button', { name: 'Split view' }).click();
  const editorScroller = page.locator('.reader-body--split .cm-scroller');
  const preview = page.locator('.reader-body--split .reader-host .mdreader');
  await expect(editorScroller).toBeVisible();
  await expect(preview).toBeVisible();
  // Let the editor lay out its full (tall) content before scrolling.
  await expect.poll(async () => editorScroller.evaluate((el) => el.scrollHeight)).toBeGreaterThan(400);

  const before = await preview.evaluate((el) => el.scrollTop);
  // Scroll the SOURCE editor to the bottom; its scroll handler maps the top
  // visible line onto the preview (one-directional, approximate).
  await editorScroller.evaluate((el) => {
    el.scrollTop = el.scrollHeight;
    el.dispatchEvent(new Event('scroll'));
  });

  // The preview followed (approximate threshold — exact block heights differ).
  await expect
    .poll(async () => preview.evaluate((el) => el.scrollTop), { timeout: 4000 })
    .toBeGreaterThan(before + 20);
});

test('navigating away from an unsaved edit PRESERVES the buffer; returning restores it', async ({
  page,
}) => {
  await page.getByRole('button', { name: /^Open…$/ }).click();
  await page.getByRole('treeitem', { name: 'index.md' }).click();
  await expect(page.getByRole('heading', { name: 'Welcome' })).toBeVisible();

  // Split mode gives us an editor AND a preview whose wikilinks we can click.
  await page.getByRole('button', { name: 'Split view' }).click();
  const editor = page.locator('.cm-content');
  await editor.click();
  const meta = await usesMeta(page);
  await page.keyboard.press(meta ? 'Meta+ArrowDown' : 'Control+End');
  await page.keyboard.type('\n\nMY UNSAVED EDIT\n');
  await expect(page.locator('.tab__dirty')).toBeVisible();

  // Click [[Guide]] in the preview → the SAME tab navigates to guide.md.
  await page
    .locator('.reader-body--split .reader-host')
    .getByRole('link', { name: 'Guide', exact: true })
    .click();
  await expect(page.getByRole('heading', { name: 'Guide' })).toBeVisible();

  // The unsaved buffer for index.md is NOT dropped: the tab still shows unsaved
  // changes, and guide.md is in its own plain read mode (no editor mounted, so a
  // save could never write index.md's buffer to guide.md).
  await expect(page.locator('.tab__dirty')).toBeVisible();
  await expect(page.locator('.cm-content')).toHaveCount(0);

  // Navigate BACK to index.md → the unsaved edit is restored in the editor.
  await page.keyboard.press(meta ? 'Meta+[' : 'Control+[');
  await expect(page.getByRole('heading', { name: 'Welcome' })).toBeVisible();
  await expect(page.locator('.cm-content')).toContainText('MY UNSAVED EDIT');
});

test('file-jump palette opens a document by name', async ({ page }) => {
  await page.getByRole('button', { name: /^Open…$/ }).click();
  await expect(page.getByRole('treeitem', { name: 'index.md' })).toBeVisible();

  // Press the same primary modifier the shell listens for (mirrors detectMac).
  const usesMeta = await page.evaluate(() => {
    const platform =
      (navigator as { userAgentData?: { platform?: string } }).userAgentData?.platform ??
      navigator.platform ??
      '';
    return /mac|iphone|ipad|ipod/i.test(platform);
  });
  await page.keyboard.press(usesMeta ? 'Meta+p' : 'Control+p');
  const input = page.getByPlaceholder('Jump to a file…');
  await expect(input).toBeVisible();
  await input.fill('refer');
  await page.keyboard.press('Enter');
  await expect(page.getByRole('heading', { name: 'Reference' })).toBeVisible();
});
