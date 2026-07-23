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

test('search intents: Best is default, Exact is selectable, Tab cycles', async ({ page }) => {
  await page.getByRole('button', { name: /^Open…$/ }).click();
  await expect(page.getByRole('treeitem', { name: 'index.md' })).toBeVisible();

  const meta = await usesMeta(page);
  await page.keyboard.press(meta ? 'Meta+k' : 'Control+k');
  await expect(page.getByRole('tab', { name: 'Best' })).toHaveAttribute('aria-selected', 'true');

  await page.getByRole('tab', { name: 'Exact' }).click();
  await expect(page.getByRole('tab', { name: 'Exact' })).toHaveAttribute('aria-selected', 'true');

  // Tab cycles to the next intent (Semantic).
  await page.getByRole('combobox').press('Tab');
  await expect(page.getByRole('tab', { name: 'Semantic' })).toHaveAttribute('aria-selected', 'true');
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
