import { expect, test } from '@playwright/test';

test.beforeEach(async ({ page }) => {
  await page.goto('/e2e.html');
});

test('Settings is a focused, local-only sheet with responsive desktop and narrow layouts', async ({
  page,
}, testInfo) => {
  await page.getByRole('button', { name: /^Open…$/ }).click();
  await expect(page.getByRole('treeitem', { name: 'index.md' })).toBeVisible();

  await page.getByRole('button', { name: 'Settings' }).click();
  const sheet = page.getByRole('dialog', { name: 'Settings' });
  await expect(sheet).toBeVisible();
  await expect(sheet.getByRole('heading', { name: 'Appearance' })).toBeVisible();
  await expect(sheet.getByRole('heading', { name: 'Library' })).toBeVisible();
  await expect(sheet.getByRole('heading', { name: 'Diagnostics' })).toBeVisible();
  await expect(sheet.getByText('Local Markdown folders', { exact: true })).toBeVisible();
  await expect(sheet.getByText('Active', { exact: true })).toBeVisible();
  await expect(sheet.getByText('Index on disk', { exact: true })).toBeVisible();
  await expect(sheet.getByText(/256 KiB per file · 3 files · 512 KiB export/)).toBeVisible();
  await expect(sheet.getByText(/Google Drive|Dropbox|Notion|connector/i)).toHaveCount(0);

  await sheet.getByRole('button', { name: 'Dark' }).click();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');

  await testInfo.attach('settings-desktop', {
    body: await page.screenshot({ animations: 'disabled' }),
    contentType: 'image/png',
  });

  await sheet.getByRole('button', { name: 'Clear logs' }).click();
  const confirm = page.getByRole('alertdialog', { name: 'Clear diagnostic logs' });
  await expect(confirm).toContainText(/cannot be undone/i);
  await confirm.getByRole('button', { name: 'Clear diagnostic logs now' }).click();
  await expect(sheet.getByRole('status')).toContainText('Diagnostic logs cleared');

  await page.setViewportSize({ width: 320, height: 640 });
  const narrowBox = await sheet.boundingBox();
  expect(narrowBox).not.toBeNull();
  expect(narrowBox!.x).toBe(0);
  expect(narrowBox!.y).toBe(0);
  expect(narrowBox!.width).toBe(320);
  expect(narrowBox!.height).toBe(640);
  await expect(sheet.getByRole('button', { name: 'Close Settings' })).toBeVisible();

  await testInfo.attach('settings-narrow-320x640', {
    body: await page.screenshot({ animations: 'disabled' }),
    contentType: 'image/png',
  });
});

test('Cmd+, opens Settings and Escape restores focus to its invoker', async ({ page }) => {
  const trigger = page.getByRole('button', { name: 'Settings' });
  await trigger.focus();

  const meta = await page.evaluate(() => {
    const platform =
      (navigator as { userAgentData?: { platform?: string } }).userAgentData?.platform ??
      navigator.platform ??
      '';
    return /mac|iphone|ipad|ipod/i.test(platform);
  });
  await page.keyboard.press(meta ? 'Meta+Comma' : 'Control+Comma');
  await expect(page.getByRole('dialog', { name: 'Settings' })).toBeVisible();

  await page.keyboard.press('Escape');
  await expect(page.getByRole('dialog', { name: 'Settings' })).toHaveCount(0);
  await expect(trigger).toBeFocused();
});
