import { expect, test } from '@playwright/test';

test.beforeEach(async ({ page }) => {
  await page.goto('/e2e.html');
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
