import { expect, test } from '@playwright/test';

test.beforeEach(async ({ page }) => {
  await page.goto('/e2e.html');
});

test('Settings categories keep local controls usable across desktop and narrow layouts', async ({
  page,
}, testInfo) => {
  await page.getByRole('button', { name: /^Open…$/ }).click();
  await expect(page.getByRole('treeitem', { name: 'index.md' })).toBeVisible();

  await page.getByRole('button', { name: 'Settings' }).click();
  const sheet = page.getByRole('dialog', { name: 'Settings' });
  await expect(sheet).toBeVisible();
  await expect(sheet.getByRole('heading', { name: 'Appearance' })).toBeVisible();
  await expect(sheet.getByRole('heading', { name: 'Account & sync' })).toBeHidden();
  await sheet.getByRole('button', { name: 'Library & sources', exact: true }).click();
  await expect(sheet.getByText('Local Markdown folders', { exact: true })).toBeVisible();
  await expect(sheet.getByText('Active', { exact: true })).toBeVisible();
  await expect(sheet.getByText('Index on disk', { exact: true })).toBeVisible();
  await sheet.getByRole('button', { name: 'Manage cloud sources' }).click();
  await expect(sheet.getByRole('button', { name: 'Choose Google Drive files' })).toBeDisabled();
  await expect(sheet.getByRole('button', { name: 'Connect Dropbox App Folder' })).toBeDisabled();
  await expect(sheet.getByText('No selected cloud sources are connected.')).toBeVisible();
  await sheet.getByRole('button', { name: 'Hide cloud sources' }).click();
  await sheet.getByRole('button', { name: 'Account & sync', exact: true }).click();
  await expect(sheet.getByText('Encrypted sync is not configured in this build.')).toBeVisible();
  await expect(sheet.getByRole('button', { name: 'Refresh status' })).toBeVisible();
  await expect(sheet.getByText('Set up sync', { exact: true })).toHaveCount(0);
  // The fixture cannot authenticate or enable recovery. Only status was inspected.
  await sheet.getByRole('button', { name: 'Appearance', exact: true }).click();
  await sheet.getByRole('button', { name: 'Dark' }).click();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');

  await testInfo.attach('settings-desktop', {
    body: await page.screenshot({ animations: 'disabled', path: testInfo.outputPath('settings-desktop.png') }),
    contentType: 'image/png',
  });

  await sheet.getByRole('button', { name: 'Advanced', exact: true }).click();
  await expect(sheet.getByRole('heading', { name: 'Diagnostics' })).toBeVisible();
  await sheet.getByRole('button', { name: 'Clear logs' }).click();
  const confirm = page.getByRole('alertdialog', { name: 'Clear diagnostic logs' });
  await expect(confirm).toContainText(/cannot be undone/i);
  await confirm.getByRole('button', { name: 'Clear diagnostic logs now' }).click();
  await expect(sheet.getByText(/^Diagnostic logs cleared\./)).toBeVisible();

  await sheet.getByRole('button', { name: 'Close Settings' }).click();
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await page.setViewportSize({ width: 320, height: 640 });
  const narrowBox = await sheet.boundingBox();
  expect(narrowBox).not.toBeNull();
  expect(narrowBox!.x).toBe(0);
  expect(narrowBox!.y).toBe(0);
  expect(narrowBox!.width).toBe(320);
  expect(narrowBox!.height).toBe(640);
  await expect(sheet.getByRole('button', { name: 'Close Settings' })).toBeVisible();
  await sheet.getByRole('combobox', { name: 'Settings section', exact: true }).selectOption('account');
  await expect(sheet.getByRole('heading', { name: 'Account & sync' })).toBeVisible();
  await expect(sheet.getByRole('heading', { name: 'Account & sync' })).toBeInViewport({ ratio: 1 });
  await expect(sheet.getByRole('button', { name: 'Refresh status' })).toBeInViewport({ ratio: 1 });
  await expect(sheet.getByText('Encrypted sync is not configured in this build.')).toBeVisible();
  expect(await sheet.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);

  await testInfo.attach('settings-narrow-320x640', {
    body: await page.screenshot({ animations: 'disabled', path: testInfo.outputPath('settings-mobile.png') }),
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
